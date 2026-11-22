//! Bounded media indexing and key operations. No HTTP or account state.
mod binary;
mod fault;
mod index;
mod keys;

pub use fault::MediaFault;
pub use index::{AudioIndex, ChunkSize, PacketIndex};
pub use keys::{ContentKey, SessionKey};

#[cfg(test)]
mod checks;
