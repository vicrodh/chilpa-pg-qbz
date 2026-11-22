//! A transfer plan owns its descriptor, key, ordering and resource limits.
pub(crate) mod lease;
mod network;

use crate::{signing::CMAF_SEED, QobuzClient};
use futures_util::{stream, StreamExt};
use media_core::{AudioIndex, ContentKey, PacketIndex, SessionKey};
pub use network::SegmentSource;
use qobuz_models::{Quality, StreamQualityInfo};
use std::{
  sync::Arc,
  time::{Duration, Instant},
};

pub const REQUEST_WINDOW: usize = 3;
const SLOT_COOLDOWN: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug)]
pub struct TransferProgress {
  pub completed: u32,
  pub total: u32,
  pub received: u64,
}

pub type Observer = Arc<dyn Fn(TransferProgress) + Send + Sync>;

pub struct DeliveryPlan {
  pub index: AudioIndex,
  pub key: ContentKey,
  pub quality: StreamQualityInfo,
  pub init_latency_ms: u64,
  template: String,
  count: u8,
  descriptor: String,
  original_init: Vec<u8>,
  source: SegmentSource,
}

pub struct StoredMedia {
  pub initialization: Vec<u8>,
  pub packets: Vec<Vec<u8>>,
  pub key: ContentKey,
  pub descriptor: String,
  pub quality: StreamQualityInfo,
}

impl DeliveryPlan {
  pub async fn prepare(api: &QobuzClient, track: u64, requested: Quality) -> Result<Self, String> {
    let location = api
      .locate_media(track, requested)
      .await
      .map_err(|error| format!("get_file_url failed: {error}"))?;
    let template = location.file.url_template.ok_or("No url_template in file/url response")?;
    let envelope = location.file.key.ok_or("No key in file/url response")?;
    let wrapping = SessionKey::derive(CMAF_SEED, &location.lease.descriptor)
      .map_err(|error| format!("Session key derivation failed: {error}"))?;
    let key =
      wrapping.open(&envelope).map_err(|error| format!("Content key unwrap failed: {error}"))?;
    let source = SegmentSource::new()?;
    let clock = Instant::now();
    let original_init = source
      .read(&template.replace("$SEGMENT$", "0"))
      .await
      .map_err(|error| format!("Failed to fetch init segment: {error}"))?;
    let init_latency_ms = clock.elapsed().as_millis() as u64;
    let index = AudioIndex::read(&original_init)
      .map_err(|error| format!("Failed to parse init segment: {error}"))?;
    let quality = StreamQualityInfo::from_raw(
      location.file.format_id.unwrap_or(requested.id()),
      location.file.sampling_rate.map(f64::from),
      location.file.bits_depth.or(location.file.bit_depth),
    );
    Ok(Self {
      index,
      key,
      quality,
      init_latency_ms,
      template,
      count: location.file.n_segments,
      descriptor: location.lease.descriptor,
      original_init,
      source,
    })
  }

  pub async fn collect(&self, observer: Option<Observer>) -> Result<Vec<u8>, String> {
    if self.index.chunks.len() != usize::from(self.count) {
      return Err("CMAF segment count disagrees with init table".into());
    }
    let length = self.index.output_length().map_err(|error| error.to_string())?;
    let mut sink = Vec::new();
    sink.try_reserve_exact(length).map_err(|_| "CMAF output allocation failed")?;
    sink.extend_from_slice(&self.index.preamble);
    let mut work = stream::iter(1..=self.count)
      .map(|number| async move {
        let address = self.template.replace("$SEGMENT$", &number.to_string());
        let packet = self.source.read(&address).await?;
        tokio::time::sleep(SLOT_COOLDOWN).await;
        Ok::<_, String>((number, packet))
      })
      .buffered(REQUEST_WINDOW);
    while let Some(result) = work.next().await {
      let (number, packet) = result?;
      let index = PacketIndex::read(&packet).map_err(|error| error.to_string())?;
      if index.length().map_err(|error| error.to_string())?
        != self.index.chunks[usize::from(number) - 1].bytes as usize
      {
        return Err("CMAF segment size disagrees with init table".into());
      }
      index.append(&packet, &self.key, &mut sink).map_err(|error| error.to_string())?;
      if let Some(notify) = &observer {
        notify(TransferProgress {
          completed: number.into(),
          total: self.count.into(),
          received: packet.len() as u64,
        });
      }
    }
    if sink.len() != length {
      return Err("CMAF assembled size mismatch".into());
    }
    Ok(sink)
  }

  pub async fn archive(self, observer: Option<Observer>) -> Result<StoredMedia, String> {
    // The unordered window reports completed transfers immediately; storage is ordered.
    let owner = &self;
    let observer = &observer;
    let counter = std::sync::atomic::AtomicU32::new(0);
    let counter = &counter;
    let mut work = stream::iter(1..=self.count)
      .map(|number| async move {
        let bytes =
          owner.source.read(&owner.template.replace("$SEGMENT$", &number.to_string())).await?;
        if let Some(notify) = observer {
          let completed = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
          notify(TransferProgress {
            completed,
            total: owner.count.into(),
            received: bytes.len() as u64,
          });
        }
        tokio::time::sleep(SLOT_COOLDOWN).await;
        Ok::<_, String>((number, bytes))
      })
      .buffer_unordered(REQUEST_WINDOW);
    let mut packets: Vec<_> = (0..self.count).map(|_| Vec::new()).collect();
    while let Some(result) = work.next().await {
      let (number, bytes) = result?;
      packets[usize::from(number) - 1] = bytes;
    }
    drop(work);
    Ok(StoredMedia {
      initialization: self.original_init,
      packets,
      key: self.key,
      descriptor: self.descriptor,
      quality: self.quality,
    })
  }
}

impl StoredMedia {
  pub fn decode(&self) -> Result<Vec<u8>, String> {
    let index = AudioIndex::read(&self.initialization).map_err(|error| error.to_string())?;
    let mut sink = index.preamble;
    for bytes in &self.packets {
      PacketIndex::read(bytes)
        .and_then(|index| index.append(bytes, &self.key, &mut sink))
        .map_err(|error| error.to_string())?;
    }
    Ok(sink)
  }
}

#[cfg(test)]
mod checks;
