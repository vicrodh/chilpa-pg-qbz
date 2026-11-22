//! Lenient deserializers for the Purchases wire models.
//!
//! Ported verbatim from `src-tauri/src/api/models.rs:8-119` so the shared,
//! frontend-agnostic models deserialize real Qobuz `/purchase/*` JSON exactly
//! as the Tauri reference path does. The purchase endpoints return loosely
//! typed JSON (numeric-or-string ids, missing booleans, occasionally malformed
//! pages); these helpers coerce rather than fail the whole response.
//!
//! NOTE: `deserialize_string_or_int` in `types.rs` returns `Option<String>` and
//! is NOT a substitute for `deserialize_string_id` (which yields a bare
//! `String`); `PurchaseAlbum.id` must be a non-optional `String`, hence the
//! distinct helper here.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::types::SearchResultsPage;

/// Lenient deserializer: if the field is present but has a wrong type, return
/// `None` instead of failing the entire response. Use for non-vital optional
/// fields.
pub fn lenient_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
  D: Deserializer<'de>,
  T: DeserializeOwned,
{
  let value = serde_json::Value::deserialize(deserializer)?;
  if value.is_null() {
    return Ok(None);
  }
  Ok(serde_json::from_value(value).ok())
}

/// Deserialize a `SearchResultsPage<T>`; on `null` OR any parse failure, return
/// an empty page (`{items:[], total:0, offset:0, limit:0}`). Never throws — a
/// malformed `albums` block yields an empty albums page while `tracks` still
/// parses.
pub fn lenient_page<'de, D, T>(deserializer: D) -> Result<SearchResultsPage<T>, D::Error>
where
  D: Deserializer<'de>,
  T: DeserializeOwned,
{
  let value = serde_json::Value::deserialize(deserializer)?;
  if value.is_null() {
    return Ok(SearchResultsPage { items: Vec::new(), total: 0, offset: 0, limit: 0 });
  }
  Ok(serde_json::from_value(value).unwrap_or(SearchResultsPage {
    items: Vec::new(),
    total: 0,
    offset: 0,
    limit: 0,
  }))
}

/// Deserialize a `SearchResultsPage<T>`, salvaging items field-by-field and
/// defaulting any missing `total`/`offset`/`limit` (the Qobuz `/radio/*` pages
/// OMIT `total`). Unlike [`lenient_page`] — which returns an EMPTY page on any
/// parse failure and would silently drop every radio track — this PRESERVES the
/// items. Ported verbatim from `src-tauri/src/api/models.rs:42-91`.
pub fn lenient_page_flexible<'de, D, T>(deserializer: D) -> Result<SearchResultsPage<T>, D::Error>
where
  D: Deserializer<'de>,
  T: DeserializeOwned,
{
  let value = serde_json::Value::deserialize(deserializer)?;
  if value.is_null() {
    return Ok(SearchResultsPage { items: Vec::new(), total: 0, offset: 0, limit: 0 });
  }

  let items: Vec<T> = value
    .get("items")
    .and_then(|v| v.as_array())
    .map(|arr| {
      arr.iter().filter_map(|item| serde_json::from_value::<T>(item.clone()).ok()).collect()
    })
    .unwrap_or_default();

  let limit =
    value.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(items.len() as u32);

  let offset = value.get("offset").and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(0);

  let total =
    value.get("total").and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(items.len() as u32);

  Ok(SearchResultsPage { items, total, offset, limit })
}

/// JSON String OR Number → `String`; anything else → `""`. Used for
/// `PurchaseAlbum.id`.
pub fn deserialize_string_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
  D: Deserializer<'de>,
{
  let value = serde_json::Value::deserialize(deserializer)?;
  Ok(match value {
    serde_json::Value::String(v) => v,
    serde_json::Value::Number(v) => v.to_string(),
    _ => String::new(),
  })
}

/// Number OR numeric String → `u64` (parse-or-0); anything else → 0. Used for
/// `PurchaseTrack.id`.
pub fn deserialize_u64_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
  D: Deserializer<'de>,
{
  let value = serde_json::Value::deserialize(deserializer)?;
  Ok(match value {
    serde_json::Value::Number(v) => v.as_u64().unwrap_or(0),
    serde_json::Value::String(v) => v.parse::<u64>().unwrap_or(0),
    _ => 0,
  })
}

/// Default `true`. Used for `downloadable` / `streamable`.
pub fn serde_true() -> bool {
  true
}

#[cfg(test)]
#[path = "checks/purchase_serde_tests.rs"]
mod checks;
