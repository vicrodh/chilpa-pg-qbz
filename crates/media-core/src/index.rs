use crate::{
  binary::{atoms, Reader},
  ContentKey, MediaFault,
};
use std::ops::Range;

const TRACK_DESCRIPTOR: [u8; 16] =
  [199, 199, 93, 240, 253, 217, 81, 233, 143, 194, 41, 113, 228, 172, 248, 210];
const SAMPLE_DESCRIPTOR: [u8; 16] =
  [59, 66, 18, 146, 86, 243, 95, 117, 146, 54, 99, 182, 154, 31, 82, 178];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkSize {
  pub bytes: u32,
  pub samples: u32,
}

pub struct AudioIndex {
  pub preamble: Vec<u8>,
  pub chunks: Vec<ChunkSize>,
}

impl AudioIndex {
  pub fn read(source: &[u8]) -> Result<Self, MediaFault> {
    let descriptor = atoms(source)
      .find(|atom| atom.tag == b"uuid" && atom.payload.starts_with(&TRACK_DESCRIPTOR))
      .ok_or(MediaFault::Layout("init segment: QBZ_INIT_UUID box not found"))?;
    let mut input = Reader::new(&descriptor.payload[16..], "track descriptor");
    input.take(26)?;
    let raw_length = input.short()? as usize;
    let raw = input.take(raw_length)?;
    let start = raw
      .windows(4)
      .position(|word| word == b"fLaC")
      .ok_or(MediaFault::Layout("init UUID payload: fLaC magic not found"))?;
    let mut preamble = raw
      .get(start..start + 42)
      .ok_or(MediaFault::Layout("init UUID payload: STREAMINFO truncated"))?
      .to_vec();
    preamble[4] |= 128;
    let mut chunks = Vec::new();
    // Older descriptors may omit the optional index or end part way through it.
    if input.remaining() > 0 {
      let identifier_length = input.number::<1>()?[0] as usize;
      if input.remaining() >= identifier_length + 2 {
        input.take(identifier_length)?;
        let count = input.short()? as usize;
        for _ in 0..count.min(input.remaining() / 8) {
          chunks.push(ChunkSize { bytes: input.word()?, samples: input.word()? });
        }
      }
    }
    Ok(Self { preamble, chunks })
  }

  pub fn output_length(&self) -> Result<usize, MediaFault> {
    self.chunks.iter().try_fold(self.preamble.len(), |sum, chunk| {
      sum.checked_add(chunk.bytes as usize).ok_or(MediaFault::Layout("CMAF output size overflow"))
    })
  }
}

struct SampleRange {
  bytes: Range<usize>,
  nonce: [u8; 8],
  encrypted: bool,
}

pub struct PacketIndex {
  media: Range<usize>,
  samples: Vec<SampleRange>,
}

impl PacketIndex {
  pub fn read(source: &[u8]) -> Result<Self, MediaFault> {
    let mut metadata = None;
    let mut media_end = source.len();
    for atom in atoms(source) {
      match atom.tag {
        b"uuid" if atom.payload.starts_with(&SAMPLE_DESCRIPTOR) => metadata = Some(atom.origin),
        b"mdat" => media_end = atom.end,
        _ => (),
      }
    }
    let origin =
      metadata.ok_or(MediaFault::Layout("audio segment: QBZ_SEGMENT_UUID box not found"))?;
    let mut input = Reader::new(&source[origin + 24..], "sample descriptor");
    input.take(4)?;
    let relative = input.word()? as usize;
    let media_start =
      origin.checked_add(relative).ok_or(MediaFault::Layout("CMAF invalid media bounds"))?;
    let width = input.number::<1>()?[0] as usize;
    let count_bytes = input.number::<3>()?;
    let count = u32::from_be_bytes([0, count_bytes[0], count_bytes[1], count_bytes[2]]) as usize;
    if count > input.remaining() / (width + 8) {
      return Err(MediaFault::Layout("CMAF sample table is truncated"));
    }
    let mut position = media_start;
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
      let size = input.word()? as usize;
      input.take(2)?;
      let encrypted = input.short()? != 0;
      let encoded_nonce = input.take(width)?;
      let mut nonce = [0; 8];
      let used = width.min(nonce.len());
      nonce[..used].copy_from_slice(&encoded_nonce[..used]);
      let end =
        position.checked_add(size).ok_or(MediaFault::Layout("CMAF frame exceeds media bounds"))?;
      samples.push(SampleRange { bytes: position..end, nonce, encrypted });
      position = end;
    }
    Ok(Self { media: media_start..media_end, samples })
  }

  pub fn length(&self) -> Result<usize, MediaFault> {
    self
      .media
      .end
      .checked_sub(self.media.start)
      .ok_or(MediaFault::Layout("CMAF invalid media bounds"))
  }

  pub fn append(
    &self,
    source: &[u8],
    key: &ContentKey,
    sink: &mut Vec<u8>,
  ) -> Result<(), MediaFault> {
    let length = self.length()?;
    let media =
      source.get(self.media.clone()).ok_or(MediaFault::Layout("CMAF invalid media bounds"))?;
    if self.samples.iter().any(|sample| sample.bytes.end > self.media.end) {
      return Err(MediaFault::Layout("CMAF frame exceeds media bounds"));
    }
    sink.try_reserve(length).map_err(|_| MediaFault::Layout("CMAF output allocation failed"))?;
    let destination = sink.len();
    sink.extend_from_slice(media);
    for sample in &self.samples {
      if sample.encrypted {
        let range = destination + sample.bytes.start - self.media.start
          ..destination + sample.bytes.end - self.media.start;
        key.apply(&sample.nonce, &mut sink[range]);
      }
    }
    Ok(())
  }
}
