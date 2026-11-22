//! API data types retained from QBZ.
pub mod artist_display;
mod discover;
pub mod lenient;
mod media;
mod metadata;
pub mod mixtape;
pub mod purchase_serde;
pub mod types;
pub use lenient::{parse_items_array, parse_items_lenient};
pub use types::*;
#[cfg(test)]
mod tests;
