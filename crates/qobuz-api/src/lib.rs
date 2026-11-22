//! Locally maintained API client, derived from QBZ.
pub mod client;
pub mod editorial;
pub mod endpoints;
pub mod error;
pub mod forbidden_breaker;
pub mod library_pages;
pub mod link_resolver;
pub mod lyrics;
pub mod media;
pub mod net_diag;
pub mod offline_gate;
pub mod performers;
pub mod purchases;
pub mod retry;
pub mod signing;

// Re-export main types
pub use client::QobuzClient;
pub use error::{ApiError, Result};
pub use link_resolver::{resolve_link, LinkResolverError, ResolvedLink};
pub use lyrics::{
  merge_translation_into, QobuzLyricsContent, QobuzLyricsDocument, QobuzLyricsLine,
  QobuzLyricsPlainLine, QobuzLyricsPublisher, QobuzLyricsTranslation, QobuzLyricsUrls,
  QobuzLyricsWord,
};
mod client_artist_page;
mod client_catalog;

mod client_detail;
mod client_headers;
mod client_labels;
mod client_library;
mod client_search;
mod client_streaming;
#[cfg(test)]
mod client_tests;

pub mod reporting;
