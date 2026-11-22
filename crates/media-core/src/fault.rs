#[derive(Debug, thiserror::Error)]
pub enum MediaFault {
  #[error("{section}: need {needed} bytes at {offset}, have {available}")]
  Bounds { section: &'static str, offset: usize, needed: usize, available: usize },
  #[error("{0}")]
  Layout(&'static str),
  #[error("Base64 decode error: {0}")]
  Encoding(#[from] base64::DecodeError),
  #[error("{0}")]
  Key(&'static str),
}
