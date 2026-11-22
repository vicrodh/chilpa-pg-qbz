use crate::MediaFault;

pub(crate) struct Reader<'a> {
  source: &'a [u8],
  offset: usize,
  section: &'static str,
}

impl<'a> Reader<'a> {
  pub fn new(source: &'a [u8], section: &'static str) -> Self {
    Self { source, offset: 0, section }
  }

  pub fn remaining(&self) -> usize {
    self.source.len() - self.offset
  }

  pub fn take(&mut self, length: usize) -> Result<&'a [u8], MediaFault> {
    let end = self.offset.checked_add(length).filter(|end| *end <= self.source.len()).ok_or(
      MediaFault::Bounds {
        section: self.section,
        offset: self.offset,
        needed: length,
        available: self.remaining(),
      },
    )?;
    let result = &self.source[self.offset..end];
    self.offset = end;
    Ok(result)
  }

  pub fn number<const N: usize>(&mut self) -> Result<[u8; N], MediaFault> {
    Ok(self.take(N)?.try_into().expect("checked width"))
  }

  pub fn short(&mut self) -> Result<u16, MediaFault> {
    Ok(u16::from_be_bytes(self.number()?))
  }
  pub fn word(&mut self) -> Result<u32, MediaFault> {
    Ok(u32::from_be_bytes(self.number()?))
  }
}

pub(crate) struct Atom<'a> {
  pub origin: usize,
  pub tag: &'a [u8],
  pub payload: &'a [u8],
  pub end: usize,
}

pub(crate) fn atoms(source: &[u8]) -> impl Iterator<Item = Atom<'_>> {
  let mut position = 0usize;
  std::iter::from_fn(move || {
    let header = source.get(position..position.checked_add(8)?)?;
    let declared = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
    let length = if declared == 0 { source.len() - position } else { declared };
    let end = position.checked_add(length)?;
    if length < 8 || end > source.len() {
      return None;
    }
    let atom =
      Atom { origin: position, tag: &header[4..], payload: &source[position + 8..end], end };
    position = end;
    Some(atom)
  })
}
