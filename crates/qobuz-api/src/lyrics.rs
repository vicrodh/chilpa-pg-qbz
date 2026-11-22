//! Qobuz lyrics wire DTOs.
//!
//! Two-step contract (inferred spec `qobuz-api-inferred-openapi-v9.9.0.0-beta.yaml:759-806`,
//! reconciled against LIVE captures 2026-06-10 — samples in
//! `qbz-nix-docs/qobuz-api/lyrics-*.json`):
//!   1. `GET /track/lyricsUrl` (signed RPC) -> [`QobuzLyricsUrls`] envelope with the CDN URL.
//!   2. `GET <lyrics_url>` (pre-signed CloudFront URL) -> [`QobuzLyricsDocument`].
//!
//! Divergences found live vs the yaml (each noted on the affected field):
//!   - Step-1 `track_id` is a JSON NUMBER (yaml said string); `album_id` IS a string.
//!   - The CDN payload is NOT the bare content union the yaml describes
//!     (`LyricsContentDto`): it is a wrapper `{album_id, track_id,
//!     translation_langs, publishers, writers, original}` with the content
//!     union under `original`.
//!   - The synced discriminator is `"wsync"` (word-synced), not `"lsync"`:
//!     each line carries per-WORD timestamps in addition to line start/end.
//!   - Plain lines are objects `{"line": "..."}`, not bare strings.
//!   - Miss = HTTP 404 with `{"status":"error","code":404,"message":"Lyrics
//!     are not available for this track."}` (sample: lyrics-miss-response.json).
//!
//! All fields stay serde-tolerant (`Option` / `#[serde(default)]`) — the
//! endpoint is a beta-era delta and may still move.

use serde::{Deserialize, Deserializer, Serialize};

/// Accept either a JSON string or a JSON number and normalize to `String`.
///
/// The yaml declares `track_id` as string ("string form, even though the query
/// param is numeric", yaml:205) but the live capture (lyrics-url-response.json)
/// shows `track_id` as a JSON NUMBER while `album_id` is a string (album slug
/// or barcode, e.g. `"gvcirtodd95kc"` / `"5052205066164"`). Tolerate both.
fn de_opt_string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
  D: Deserializer<'de>,
{
  #[derive(Deserialize)]
  #[serde(untagged)]
  enum StringOrNumber {
    String(String),
    Number(serde_json::Number),
  }

  Ok(Option::<StringOrNumber>::deserialize(deserializer)?.map(|v| match v {
    StringOrNumber::String(s) => s,
    StringOrNumber::Number(n) => n.to_string(),
  }))
}

/// Step-1 envelope returned by `GET /track/lyricsUrl`.
///
/// Yaml `LyricsUrlsDto` (`:194-219`). Live shape (lyrics-url-response.json):
/// `{"track_id": 266725027, "album_id": "gvcirtodd95kc", "lyrics_url": "https://…cloudfront.net/…"}`
/// — `track_id` is a number (yaml said string), `translation_requested` is
/// simply ABSENT when no translation was asked, and the URL is a pre-signed
/// CloudFront URL (self-authorizing: `Expires`/`Signature`/`Key-Pair-Id`).
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsUrls {
  #[serde(default, deserialize_with = "de_opt_string_or_number")]
  pub track_id: Option<String>,
  #[serde(default, deserialize_with = "de_opt_string_or_number")]
  pub album_id: Option<String>,
  /// URL to the original-language lyrics document (Qobuz CDN host).
  #[serde(default)]
  pub lyrics_url: Option<String>,
  /// Present only when the `language` query param was sent AND a matching
  /// translation exists (yaml:208-213). Absent when no translation was
  /// requested (confirmed absent in the live capture) or none is available
  /// for the requested language.
  #[serde(default)]
  pub translation_requested: Option<QobuzLyricsTranslation>,
}

/// Translation pointer inside [`QobuzLyricsUrls`] (yaml `LyricsTranslationDto`, `:221-231`).
/// Kept yaml-shaped + tolerant.
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsTranslation {
  #[serde(default)]
  pub lyrics_url: Option<String>,
  /// ISO 639-1 code of the returned translation.
  #[serde(default)]
  pub lang: Option<String>,
}

/// Step-2 CDN lyrics document — the REAL wire shape (live divergence: the
/// yaml's `LyricsContentDto` union is nested under `original`, wrapped with
/// catalog metadata; samples lyrics-doc-wsync.json / lyrics-doc-plain.json).
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsDocument {
  #[serde(default, deserialize_with = "de_opt_string_or_number")]
  pub track_id: Option<String>,
  #[serde(default, deserialize_with = "de_opt_string_or_number")]
  pub album_id: Option<String>,
  /// ISO 639-1 codes a translation exists for (e.g. `["pt","de","fr","es","it"]`).
  #[serde(default)]
  pub translation_langs: Vec<String>,
  /// Publisher copyright entries with applicability zones. Parsed but never
  /// enforced (owner decision 2026-07-22: no zone/copyright gating).
  #[serde(default)]
  pub publishers: Vec<QobuzLyricsPublisher>,
  /// Songwriter credits as a single display string.
  #[serde(default)]
  pub writers: Option<String>,
  /// The actual lyrics content (the yaml's "LyricsContentDto" union).
  #[serde(default)]
  pub original: Option<QobuzLyricsContent>,
  /// Translated lyrics content (NEW in API v10) — same union shape as
  /// `original`. LIVE WIRE (verified 2026-07-22): when `language` is
  /// requested and available, the step-1 `lyrics_url` itself points at the
  /// TRANSLATED document (`<id>-<lang>.json`), which carries ONLY this
  /// `translation` block and no `original` — the client must fetch the
  /// original document separately and merge. `translation_requested` was
  /// not observed in practice (kept as a fallback source only).
  #[serde(default)]
  pub translation: Option<QobuzLyricsContent>,
}

/// Merge a translation-only document into its original document (v10 live
/// wire: a `language` request returns a document carrying ONLY `translation`).
/// Moves the `translation` block over and unions `translation_langs`.
/// `original` keeps its own content/writers/publishers.
pub fn merge_translation_into(
  original: &mut QobuzLyricsDocument,
  mut translated: QobuzLyricsDocument,
) {
  original.translation = translated.translation.take();
  for lang in translated.translation_langs.drain(..) {
    if !original.translation_langs.contains(&lang) {
      original.translation_langs.push(lang);
    }
  }
}

/// One publisher credit inside [`QobuzLyricsDocument`] (live-only; not in the yaml).
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsPublisher {
  #[serde(default)]
  pub copyright: Option<String>,
  /// Zone codes the copyright applies to (e.g. `["WW"]`, `["WW","FR"]`).
  #[serde(default)]
  pub zones: Vec<String>,
}

/// Lyrics content union, discriminated on `type`.
///
/// Live discriminator is `"wsync"` (word-synced — richer than the yaml's
/// `"lsync"` guess: per-word timestamps inside each line). `"lsync"` is kept
/// as an alias in case line-only documents exist in the catalog (same shape
/// minus `words`, which defaults to empty). `Serialize` exists so the lyrics
/// cache can persist a document's embedded translation verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum QobuzLyricsContent {
  /// Time-synced lyrics; line + per-word timestamps, ms from track start.
  #[serde(rename = "wsync", alias = "lsync")]
  Synced {
    /// ISO 639-1 of the document language (present in live captures).
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    lines: Vec<QobuzLyricsLine>,
  },
  /// Plain (non-synced) lyrics: text-only lines, no timestamps.
  #[serde(rename = "plain")]
  Plain {
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    lines: Vec<QobuzLyricsPlainLine>,
  },
}

impl QobuzLyricsContent {
  /// True when this document carries per-line timestamps.
  pub fn is_synced(&self) -> bool {
    matches!(self, QobuzLyricsContent::Synced { .. })
  }

  /// Number of lines in the document.
  pub fn line_count(&self) -> usize {
    match self {
      QobuzLyricsContent::Synced { lines, .. } => lines.len(),
      QobuzLyricsContent::Plain { lines, .. } => lines.len(),
    }
  }

  /// ISO 639-1 of the content's language, when the document carries it.
  pub fn lang(&self) -> Option<&str> {
    match self {
      QobuzLyricsContent::Synced { lang, .. } => lang.as_deref(),
      QobuzLyricsContent::Plain { lang, .. } => lang.as_deref(),
    }
  }
}

/// One synced line.
///
/// Live divergence from the yaml's `LyricsLineDto` (`{line,start,end}` all
/// required): instrumental-gap separator lines come as `{"line":"","words":[]}`
/// with NO `start`/`end` at all — both must be `Option`. Regular lines carry
/// line-level `start`/`end` plus per-word stamps (lyrics-doc-wsync.json).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QobuzLyricsLine {
  #[serde(default)]
  pub line: String,
  /// Per-word timestamps ("wsync"); empty for gap lines and "lsync" docs.
  #[serde(default)]
  pub words: Vec<QobuzLyricsWord>,
  /// Line start, ms from track start. Absent on gap lines.
  #[serde(default)]
  pub start: Option<i64>,
  /// Line end, ms from track start. Absent on gap lines.
  #[serde(default)]
  pub end: Option<i64>,
}

/// One word inside a synced line (live-only; not in the yaml).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QobuzLyricsWord {
  #[serde(default)]
  pub word: String,
  /// Word start, ms from track start.
  #[serde(default)]
  pub start: i64,
  /// Word end, ms from track start.
  #[serde(default)]
  pub end: i64,
}

/// One plain line. Live wire form is an object `{"line": "..."}`
/// (lyrics-doc-plain.json); the yaml guessed bare strings (`:287-303`) —
/// accept both.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QobuzLyricsPlainLine {
  pub line: String,
}

impl<'de> Deserialize<'de> for QobuzLyricsPlainLine {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
      Text(String),
      Object {
        #[serde(default)]
        line: String,
      },
    }

    Ok(match Repr::deserialize(deserializer)? {
      Repr::Text(line) => QobuzLyricsPlainLine { line },
      Repr::Object { line } => QobuzLyricsPlainLine { line },
    })
  }
}

#[cfg(test)]
#[path = "checks/lyrics_tests.rs"]
mod checks;
