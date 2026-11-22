//! Core API types for QBZ
//!
//! This module contains all shared data types used across the application:
//! - Media types: Track, Album, Artist, Playlist
//! - Quality/streaming types
//! - Search and favorites types
//! - Image and metadata types

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ============ Dynamic-suggest (DailyQ/WeeklyQ) ============

/// A seed track resolved for the `/dynamic/suggest` `track_to_analysed`
/// payload (DailyQ / WeeklyQ). Field names match the Qobuz wire shape
/// exactly; `0` marks an unknown id (mirrors Tauri's `?? 0`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackToAnalyse {
  pub track_id: u64,
  pub artist_id: u64,
  pub genre_id: u64,
  pub label_id: u64,
}

// ============ Quality Types ============

/// Audio quality format IDs (matches Qobuz API format IDs)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u32)]
pub enum Quality {
  Mp3 = 5,
  Lossless = 6,    // 16-bit/44.1kHz (CD Quality)
  HiRes = 7,       // 24-bit/≤96kHz
  UltraHiRes = 27, // 24-bit/>96kHz
}

impl Quality {
  pub fn from_id(id: u32) -> Option<Self> {
    match id {
      5 => Some(Quality::Mp3),
      6 => Some(Quality::Lossless),
      7 => Some(Quality::HiRes),
      27 => Some(Quality::UltraHiRes),
      _ => None,
    }
  }

  pub fn id(&self) -> u32 {
    *self as u32
  }

  pub fn label(&self) -> &'static str {
    match self {
      Quality::Mp3 => "MP3 320kbps",
      Quality::Lossless => "FLAC 16-bit/44.1kHz",
      Quality::HiRes => "FLAC 24-bit/≤96kHz",
      Quality::UltraHiRes => "FLAC 24-bit/>96kHz",
    }
  }

  /// Quality levels in descending order for fallback
  pub fn fallback_order() -> &'static [Quality] {
    &[Quality::UltraHiRes, Quality::HiRes, Quality::Lossless, Quality::Mp3]
  }

  /// Returns the next lower quality level, or None if already at the lowest (Mp3).
  /// Used for CDN fallback when a quality level consistently fails.
  pub fn lower(&self) -> Option<Quality> {
    match self {
      Quality::UltraHiRes => Some(Quality::HiRes),
      Quality::HiRes => Some(Quality::Lossless),
      Quality::Lossless => Some(Quality::Mp3),
      Quality::Mp3 => None,
    }
  }

  /// The lower of two tiers. Implementable as plain `min` because the
  /// derived `Ord` on `Quality` is tier-correct: the Qobuz format-id
  /// discriminants (5 Mp3 < 6 Lossless < 7 HiRes < 27 UltraHiRes) ascend
  /// with tier. Used to clamp a requested tier against a cap (#638).
  pub fn min_tier(a: Quality, b: Quality) -> Quality {
    a.min(b)
  }
}

impl Default for Quality {
  fn default() -> Self {
    Quality::Lossless
  }
}

/// Why a delivered stream is (or may be) below the track's catalog maximum.
/// Shared by the local badge, the local device cap, and the cast surfaces
/// (#638 fixes 1-4). Mirrored to Slint as a plain `int` property carrying the
/// same discriminant — no string enum crosses the FFI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualityLimit {
  /// No constraint identified (or no downgrade).
  #[default]
  None = 0,
  /// The user's streaming-quality preference capped the request.
  Preference = 1,
  /// The local output device's cap lowered the request (fix 3).
  /// NEVER applicable while casting — the local DAC is not in a cast's
  /// signal path (precedence rule, owner decision 2026-07-20).
  LocalDeviceCap = 2,
  /// The manual per-renderer cap lowered the request (fix 4). Cast only.
  RendererCap = 3,
  /// Qobuz did not offer a higher tier for this track.
  CatalogAvailability = 4,
}

// ============ User Session ============

/// What the account is entitled to, straight from
/// `user.credential.parameters` of the `/user/login` payload (wire names,
/// verified against Qobuz's own embedded fixture — see
/// `qbz-nix-docs/offline-mode/tauri-review-2026-06-09/10-subscription-trial-offline-gating.md`
/// §1.2). An account without an active subscription ("Qobuz member") has
/// no `parameters` at all and therefore every flag `false`; Qobuz still
/// serves it favorites, playlists, purchases and 30-second previews.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entitlements {
  #[serde(default)]
  pub lossy_streaming: bool,
  #[serde(default)]
  pub lossless_streaming: bool,
  #[serde(default)]
  pub hires_streaming: bool,
  #[serde(default)]
  pub hires_purchases_streaming: bool,
  #[serde(default)]
  pub offline_streaming: bool,
  #[serde(default)]
  pub mobile_streaming: bool,
}

impl Entitlements {
  /// No streaming entitlement of any kind: a Qobuz member without a
  /// subscription. Purchases and previews still work; full-length
  /// catalog playback and offline do not.
  pub fn is_member_only(&self) -> bool {
    !(self.lossy_streaming || self.lossless_streaming || self.hires_streaming)
  }
}

/// User credentials and session info
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserSession {
  pub user_auth_token: String,
  pub user_id: u64,
  pub email: String,
  pub display_name: String,
  pub subscription_label: String,
  #[serde(default)]
  pub subscription_valid_until: Option<String>,
  /// Entitlement flags from `credential.parameters`. `serde(default)`
  /// keeps persisted pre-member-mode sessions loadable (all `false`,
  /// which the next real login corrects).
  #[serde(default)]
  pub entitlements: Entitlements,
  /// `user.subscription.end_date` (`YYYY-MM-DD`), when Qobuz sends the
  /// `subscription` object at all. Informational: the entitlement flags
  /// are the server's verdict, not this date (the embedded fixture has a
  /// past `end_date` with every flag still `true`).
  #[serde(default)]
  pub subscription_end_date: Option<String>,
  /// `user.subscription.offer` (e.g. "studio", "sublime"), when present.
  #[serde(default)]
  pub subscription_offer: Option<String>,
  /// Account territory (ISO 3166-1 alpha-2, e.g. "FR") from the login
  /// response. `serde(default)` keeps pre-v10 persisted sessions loadable.
  #[serde(default)]
  pub country_code: Option<String>,
  /// Account language (ISO 639-1, e.g. "fr") from the login response —
  /// the default target for lyrics translation ("Auto").
  #[serde(default)]
  pub language_code: Option<String>,
}

// ============ Stream Types ============

/// Stream URL response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamUrl {
  /// Opaque reporting material returned by the service, never synthesized.
  #[serde(default)]
  pub blob: Option<String>,
  #[serde(default)]
  pub duration: Option<f64>,
  pub url: String,
  pub format_id: u32,
  pub mime_type: String,
  pub sampling_rate: f64,
  pub bit_depth: Option<u32>,
  pub track_id: u64,
  pub restrictions: Vec<StreamRestriction>,
  /// `true` when Qobuz served a 30-second preview instead of the track
  /// (an account without the streaming entitlement, or a track that is
  /// not streamable in the territory). The URL still plays; the player
  /// must say so and must not count it as a listen.
  #[serde(default)]
  pub sample: bool,
}

impl StreamUrl {
  /// Check if the stream has restrictions that prevent playback
  pub fn has_restrictions(&self) -> bool {
    self.restrictions.iter().any(|r| {
      r.code == "FormatRestrictedByFormatAvailability" || r.code == "SampleRestrictedByRightHolders"
    })
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRestriction {
  pub code: String,
}

// ============ External streaming (Cast / DLNA) ============

/// Resolved audio quality actually delivered for an external stream, in the
/// kHz convention used across the catalog and [`StreamUrl`]. Surfaced so the
/// UI can show the REAL quality of a cast stream, which can fall back below
/// the requested tier (HiRes -> Lossless -> Mp3) without the user knowing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamQualityInfo {
  /// Qobuz format id: 5=MP3, 6=Lossless, 7=HiRes, 27=UltraHiRes.
  pub format_id: u32,
  /// Sampling rate in kHz (e.g. 96.0, 192.0), when known.
  pub sampling_rate_khz: Option<f64>,
  /// Bit depth (16 / 24), when known.
  pub bit_depth: Option<u32>,
}

impl StreamQualityInfo {
  /// Build from a raw sampling-rate value whose unit may be kHz or Hz
  /// depending on the Qobuz endpoint (`get_stream_url` reports kHz as f64,
  /// `file/url` reports an integer that has been observed as kHz). Normalize
  /// to kHz robustly: any real audio rate is < 1000 kHz and >= 8000 Hz, so a
  /// value >= 1000 is Hz and gets divided. Zero/negative -> unknown.
  pub fn from_raw(format_id: u32, raw_rate: Option<f64>, bit_depth: Option<u32>) -> Self {
    let sampling_rate_khz = raw_rate.and_then(|r| {
      if r <= 0.0 {
        None
      } else if r >= 1000.0 {
        Some(r / 1000.0)
      } else {
        Some(r)
      }
    });
    Self { format_id, sampling_rate_khz, bit_depth }
  }

  /// The `Quality` tier this format id maps to, if recognized.
  pub fn quality(&self) -> Option<Quality> {
    Quality::from_id(self.format_id)
  }

  /// Coarse tier label like "FLAC 24-bit/>96kHz" (from the format id).
  pub fn tier_label(&self) -> &'static str {
    self.quality().map(|q| q.label()).unwrap_or("Unknown")
  }
}

/// Measured stream parameters read from the head of an audio buffer
/// (FLAC STREAMINFO). `sample_rate` is in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioParams {
  pub sample_rate: u32,
  pub bits_per_sample: u32,
  pub channels: u16,
}

/// Parse a FLAC STREAMINFO block from the head of a stream. Returns `None`
/// for non-FLAC or short buffers — never guesses defaults (callers that need
/// a fallback keep their own). Bit math hoisted verbatim from the proven
/// Remote stream probe, shared so the cast
/// path can measure the bytes it actually serves (#638 fix 1).
pub fn probe_streaminfo(bytes: &[u8]) -> Option<AudioParams> {
  if bytes.len() >= 26 && bytes.starts_with(b"fLaC") {
    let sample_rate =
      ((bytes[18] as u32) << 12) | ((bytes[19] as u32) << 4) | ((bytes[20] as u32) >> 4);
    let channels = ((bytes[20] >> 1) & 0x07) + 1;
    let bit_depth = ((bytes[20] & 0x01) << 4) | ((bytes[21] >> 4) & 0x0F);
    Some(AudioParams {
      sample_rate,
      bits_per_sample: (bit_depth + 1) as u32,
      channels: channels as u16,
    })
  } else {
    None
  }
}

/// Where the bytes for an external/cast track were resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetOrigin {
  Network,
  Cache,
  Offline,
}

/// A fully-materialized audio asset to hand to an external renderer
/// (Chromecast / DLNA) through the local media server. Carries the raw bytes
/// VERBATIM (no transcode), the MIME to advertise, and the quality actually
/// resolved so the UI can display it. Casting bypasses the local audio
/// backend, so this is the only place the delivered quality is known.
#[derive(Clone)]
pub struct ExternalStreamAsset {
  pub bytes: Vec<u8>,
  pub content_type: String,
  pub quality: StreamQualityInfo,
  /// Track duration in seconds, when known by the resolver.
  pub duration_secs: Option<f64>,
  pub origin: AssetOrigin,
}

impl std::fmt::Debug for ExternalStreamAsset {
  // Don't dump the whole byte vec into logs.
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ExternalStreamAsset")
      .field("bytes", &format_args!("{} bytes", self.bytes.len()))
      .field("content_type", &self.content_type)
      .field("quality", &self.quality)
      .field("duration_secs", &self.duration_secs)
      .field("origin", &self.origin)
      .finish()
  }
}

// ============ CMAF Stream Types ============

/// Response from POST /api.json/0.2/session/start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartResponse {
  pub session_id: String,
  pub expires_at: u64,
  #[serde(default)]
  pub infos: Option<String>,
}

/// Response from GET /api.json/0.2/file/url (CMAF segmented streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackFileUrl {
  #[serde(default)]
  pub url_template: Option<String>,
  #[serde(default)]
  pub url: Option<String>,
  #[serde(default)]
  pub mime_type: Option<String>,
  #[serde(default)]
  pub n_segments: u8,
  #[serde(default)]
  pub key_id: Option<String>,
  #[serde(default)]
  pub key: Option<String>,
  #[serde(default)]
  pub sampling_rate: Option<u32>,
  #[serde(default)]
  pub bit_depth: Option<u32>,
  #[serde(default)]
  pub bits_depth: Option<u32>,
  #[serde(default)]
  pub duration: Option<f64>,
  #[serde(default)]
  pub n_samples: Option<u64>,
  #[serde(default)]
  pub format_id: Option<u32>,
  #[serde(default)]
  pub track_id: Option<u64>,
  #[serde(default)]
  pub restrictions: Vec<StreamRestriction>,
}

// ============ Image Types ============

/// Pixel size each `ImageSet` variant serves, ascending: small, thumbnail,
/// large, extralarge, mega. This is the Qobuz CDN CONVENTION — the API does
/// not state variant dimensions in-repo (contract
/// `2026-08-15-immersive-completion` 00 §8: verify against live responses
/// before retuning). THE table lives here and only here: `ImageSet::for_px`
/// and the size bucketing (`ImageSet::bucket_for_px`, used by the Qt
/// frontend's responsive-art request path) both read it, so tuning either
/// side is ONE edit.
pub const IMAGE_VARIANT_PX: [u32; 5] = [50, 150, 300, 600, 3000];

/// Image set with multiple resolutions
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageSet {
  pub medium: Option<String>,
  pub small: Option<String>,
  pub thumbnail: Option<String>,
  pub large: Option<String>,
  pub extralarge: Option<String>,
  pub mega: Option<String>,
  pub back: Option<String>,
}

impl ImageSet {
  pub fn best(&self) -> Option<&String> {
    self
      .mega
      .as_ref()
      .or(self.extralarge.as_ref())
      .or(self.large.as_ref())
      .or(self.medium.as_ref())
      .or(self.thumbnail.as_ref())
      .or(self.small.as_ref())
  }

  /// The smallest variant whose pixel size (per [`IMAGE_VARIANT_PX`])
  /// covers `px` — the size-aware pick for a surface that knows its slot
  /// size, so a 72px row never fetches a ~3000px mega and a big slot never
  /// settles for a 150px thumbnail. A missing variant keeps the scan
  /// moving UP (a bigger variant still covers the slot); when nothing is
  /// big enough (or the set is partial — Discover returns only
  /// small/thumbnail/large) the fallbacks are `best()` and then
  /// `smallest()` order, so this never returns WORSE than what a bare
  /// `best()` call would have served.
  pub fn for_px(&self, px: u32) -> Option<&String> {
    let variants = [
      (&self.small, IMAGE_VARIANT_PX[0]),
      (&self.thumbnail, IMAGE_VARIANT_PX[1]),
      (&self.large, IMAGE_VARIANT_PX[2]),
      (&self.extralarge, IMAGE_VARIANT_PX[3]),
      (&self.mega, IMAGE_VARIANT_PX[4]),
    ];
    for (slot, slot_px) in variants {
      if slot_px >= px {
        if let Some(url) = slot.as_ref() {
          return Some(url);
        }
      }
    }
    self.best().or_else(|| self.smallest())
  }

  /// The size bucket a slot of `px` pixels resolves to: the smallest
  /// [`IMAGE_VARIANT_PX`] entry that covers it, or the mega entry when none
  /// does. Callers that re-resolve art on window resize compare BUCKETS, not
  /// pixels, so a resize drag costs at most one re-request per tier crossed
  /// instead of one per pixel.
  pub fn bucket_for_px(px: u32) -> u32 {
    IMAGE_VARIANT_PX
      .iter()
      .copied()
      .find(|&entry| entry >= px)
      .unwrap_or(IMAGE_VARIANT_PX[IMAGE_VARIANT_PX.len() - 1])
  }

  /// The smallest available variant — for list-row thumbnails, where
  /// `best()` (mega/large) would needlessly download huge images.
  pub fn smallest(&self) -> Option<&String> {
    self
      .small
      .as_ref()
      .or(self.thumbnail.as_ref())
      .or(self.large.as_ref())
      .or(self.extralarge.as_ref())
      .or(self.mega.as_ref())
  }
}

/// Rewrite a sized Qobuz CDN cover url to the variant tier covering `px`.
///
/// Qobuz covers carry the variant's pixel size as the `_<px>.<ext>` suffix
/// (`https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_50.jpg`),
/// with the sizes of [`IMAGE_VARIANT_PX`] — EXCEPT the mega tier, whose
/// suffix is the literal `_max` (the size it serves is the album master's,
/// 1400px and up). The queue, `recently_played.json`
/// and the persisted session store WHATEVER variant the building surface
/// picked, so a restored session can carry the 50px `small` into the
/// now-playing feed, MPRIS and Discord — measured 2026-08-15 on the owner's
/// cache: 50x50 JPEGs downloaded by the Qt now-playing art feed, pixelated
/// art on every big surface.
///
/// The rewrite targets the smallest table entry covering `px`
/// ([`ImageSet::bucket_for_px`]), so any surface holding ANY sized Qobuz
/// cover can serve itself the tier it needs without the original
/// `ImageSet`. Returns `None` — leaving the url to the caller unchanged —
/// for non-Qobuz urls, unsized covers, and suffixes the table does not know
/// (a stray `_<digits>` in an id is never mistaken for a size).
pub fn qobuz_cover_at_px(url: &str, px: u32) -> Option<String> {
  if !url.contains("/images/covers/") {
    return None;
  }
  let (stem, ext) = url.rsplit_once('.')?;
  if ext.len() > 4 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
    return None;
  }
  let (head, size) = stem.rsplit_once('_')?;
  // The mega tier's suffix is the LITERAL `_max`, not `_3000`: measured
  // against the live CDN 2026-08-15, `_3000.jpg` is a 404 on albums whose
  // mega exists (`_max.jpg` serves it, at whatever size that album's
  // master is — 1400px and up), and the API advertises mega urls with the
  // `_max` suffix. `3000` stays the tier's pixel CONVENTION in
  // [`IMAGE_VARIANT_PX`]; only the wire suffix is `max`.
  if size != "max" && !IMAGE_VARIANT_PX.contains(&size.parse::<u32>().ok()?) {
    return None;
  }
  let bucket = ImageSet::bucket_for_px(px);
  if bucket == IMAGE_VARIANT_PX[IMAGE_VARIANT_PX.len() - 1] {
    Some(format!("{head}_max.{ext}"))
  } else {
    Some(format!("{head}_{bucket}.{ext}"))
  }
}

// ============ Core Media Types ============

/// Track model
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Track {
  #[serde(default)]
  pub id: u64,
  /// Favourites-list rows only (unix seconds); see `Album::favorited_at`.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub favorited_at: Option<i64>,
  /// Store flag from the catalog (`purchasable`): `Some(false)` is the API
  /// saying this release is not sold on Qobuz; absence is unknown, not no.
  #[serde(default)]
  pub purchasable: Option<bool>,
  #[serde(default)]
  pub title: String,
  /// Subtitle/edition info from Qobuz (e.g. "Player's Ball Mix",
  /// "Nine Inch Noize Version", "Remastered 2024"). Frontend renders
  /// it parenthesized after the title so remix and reissue albums are
  /// distinguishable from originals (issue #360).
  pub version: Option<String>,
  /// Classical "work" the track belongs to (e.g. "Symphony No. 9 in D minor,
  /// Op. 125"). Qobuz returns it on the track object (always present in the
  /// envelope, `null` for non-classical catalog). Drives the per-work section
  /// headers on the album view, mirroring the official Qobuz player (PR #536).
  pub work: Option<String>,
  pub isrc: Option<String>,
  /// `audio_info` on the wire: replaygain gain/peak (always present in the
  /// captured samples). Absent on older cached payloads → `None`.
  #[serde(default)]
  pub audio_info: Option<DiscoverAudioInfo>,
  #[serde(default)]
  pub duration: u32,
  #[serde(default)]
  pub track_number: u32,
  pub media_number: Option<u32>,
  pub performer: Option<Artist>,
  pub album: Option<AlbumSummary>,
  #[serde(default)]
  pub hires: bool,
  #[serde(default)]
  pub hires_streamable: bool,
  pub maximum_sampling_rate: Option<f64>,
  pub maximum_bit_depth: Option<u32>,
  /// Whether Qobuz will stream this track today.
  ///
  /// `None` means the endpoint did not report availability — it is **NOT** a
  /// claim of unavailability. Only `Some(false)` marks a track as pulled from
  /// the catalogue. The distinction is load-bearing: this field used to be a
  /// plain `bool` with a bare `#[serde(default)]`, and that collapses an
  /// ABSENT key into `false`, so every track returned by an endpoint that
  /// omits it came back marked unavailable. That was harmless while nothing
  /// rendered the flag; it stops being harmless the moment the flag greys a
  /// row out and keeps it out of the queue, where a single terse endpoint
  /// would silently take out an entire view.
  ///
  /// `/album/get` does send the key (captured 2026-08-17, album
  /// `0886443985094`, where track 1 is `false` while the ALBUM around it is
  /// `true` — the album-level flag can never stand in for this one).
  /// `/playlist/get?extra=tracks` has never been captured, and playlists are
  /// where dead tracks show up most; that unverified endpoint is exactly what
  /// this `Option` guards against.
  ///
  /// Never read the field directly — go through [`Track::is_streamable`],
  /// which is the one place absence is interpreted.
  ///
  /// Mirrors [`Album::streamable`], which has been `Option<bool>` all along
  /// for the same reasons; the two shapes are now consistent.
  #[serde(default)]
  pub streamable: Option<bool>,
  /// Unix seconds at which streaming rights START for this track
  /// (`/album/get` sends it on every item: a PAST value on live tracks,
  /// `null` on a pulled one, a FUTURE value on a pre-release). Read through
  /// [`Track::is_upcoming`], which is what tells "not yet" from "no longer".
  #[serde(default)]
  pub streamable_at: Option<i64>,
  /// Per-track streaming release date (ISO `YYYY-MM-DD`), the same shape as
  /// [`Album::release_date_stream`]. Fallback for [`Track::is_upcoming`]
  /// when `streamable_at` is absent.
  #[serde(default)]
  pub release_date_stream: Option<String>,
  #[serde(default)]
  pub parental_warning: bool,
  /// Playlist-specific: ID within the playlist (for removal)
  pub playlist_track_id: Option<u64>,
  /// Performers/credits string (format: "Name, Role - Name, Role")
  pub performers: Option<String>,
  /// Composer information
  pub composer: Option<Artist>,
  /// Copyright information
  pub copyright: Option<String>,
}

impl Track {
  /// The ONE place an absent `streamable` is interpreted: **absent means
  /// available**.
  ///
  /// The asymmetry is deliberate, and it is the whole reason the field is an
  /// `Option` rather than a `bool` with a convenient default.
  ///
  /// Refusing to play a track because an endpoint was terse is strictly worse
  /// than offering one that turns out to be dead. The second case is already
  /// caught downstream by the reactive path — the play fails, the error is
  /// recognised as terminal (`is_terminal_unavailable`), and the bounded
  /// auto-skip moves on — so it costs one failed round trip and one toast,
  /// and the user hears the next song. The first case has no recovery at all:
  /// the user is shown a greyed, unplayable catalogue, no request is ever
  /// made, nothing fails loudly, and there is no signal anywhere that would
  /// tell them (or us) that the data was merely missing rather than the music
  /// gone. On a music player, silent refusal is the unrecoverable failure and
  /// a wasted request is the cheap one.
  ///
  /// Call this instead of touching `streamable` directly. A hand-written
  /// `unwrap_or` at a call site is how the two halves of this rule drift
  /// apart, and the drift is invisible until a whole view goes grey.
  pub fn is_streamable(&self) -> bool {
    self.streamable.unwrap_or(true)
  }

  /// Whether this track is a PRE-RELEASE: not streamable today because its
  /// streaming rights have not started yet, as opposed to a track Qobuz
  /// pulled. Only meaningful alongside `!is_streamable()`; a streamable
  /// track is never "upcoming". See [`upcoming_from`] for the rule.
  pub fn is_upcoming(&self) -> bool {
    !self.is_streamable()
      && upcoming_from(self.streamable_at, self.release_date_stream.as_deref(), chrono::Utc::now())
  }
}

/// The ONE rule that tells "not yet available" from "no longer available":
/// a FUTURE `streamable_at` (Unix seconds) wins; without it, a FUTURE
/// `release_date_stream` (`YYYY-MM-DD`, compared by calendar day in UTC).
/// Anything absent, malformed, or in the past reads as NOT upcoming, so a
/// terse endpoint keeps the existing "no longer available" wording rather
/// than promising a release that never comes.
pub fn upcoming_now(streamable_at: Option<i64>, release_date_stream: Option<&str>) -> bool {
  upcoming_from(streamable_at, release_date_stream, chrono::Utc::now())
}

/// [`upcoming_now`] against an explicit clock (tests).
pub fn upcoming_from(
  streamable_at: Option<i64>,
  release_date_stream: Option<&str>,
  now: chrono::DateTime<chrono::Utc>,
) -> bool {
  if let Some(at) = streamable_at {
    return at > now.timestamp();
  }
  release_date_stream
    .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
    .map(|date| date > now.date_naive())
    .unwrap_or(false)
}

/// Album summary (embedded in track responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumSummary {
  #[serde(default)]
  pub id: String,
  #[serde(default)]
  pub title: String,
  #[serde(default)]
  pub image: ImageSet,
  /// Label (if returned in track response)
  pub label: Option<Label>,
  /// Genre (when returned, e.g. on favorites track album objects).
  pub genre: Option<Genre>,
  /// The album's MAIN artist, when the track's embedded album carries it
  /// (search results and favourites do — captured
  /// `qobuz-api/search-results-response.json`). Distinct from the track's
  /// `performer`, which is who is credited on THAT track; the artist page's
  /// "In library" tab admits a track through either.
  #[serde(default)]
  pub artist: Option<Artist>,
  /// Release/edition suffix on embedded album rows (for example
  /// "Remastered 2021"). Search and favourites payloads carry it even
  /// though older endpoints omit it.
  #[serde(default)]
  pub version: Option<String>,
  /// Availability of the release that owns this track. As with the full
  /// [`Album`] model, absence means unknown — never unavailable.
  #[serde(default)]
  pub streamable: Option<bool>,
  /// Unix timestamp at which this release becomes streamable, when supplied
  /// by the embedding endpoint.
  #[serde(default)]
  pub streamable_at: Option<i64>,
  /// ISO streaming release date, used as the fallback upcoming signal.
  #[serde(default)]
  pub release_date_stream: Option<String>,
  /// Original release date, when included by the embedded album endpoint.
  #[serde(default)]
  pub release_date_original: Option<String>,
  /// Release classification when included by the embedding endpoint.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub release_type: Option<String>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub product_type: Option<String>,
}

impl AlbumSummary {
  /// `None` is a terse endpoint, not a withdrawal.
  pub fn is_streamable(&self) -> bool {
    self.streamable.unwrap_or(true)
  }

  /// Distinguish a future release from a withdrawn one using the same rule
  /// as [`Track::is_upcoming`].
  pub fn is_upcoming(&self) -> bool {
    !self.is_streamable()
      && upcoming_from(self.streamable_at, self.release_date_stream.as_deref(), chrono::Utc::now())
  }
}

/// Album model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
  #[serde(default)]
  pub id: String,
  /// When the user favourited it — `favorite/getUserFavorites` items carry
  /// it (unix seconds); every other album shape leaves it `None`.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub favorited_at: Option<i64>,
  /// Store flag from the catalog (`purchasable`): `Some(false)` is the API
  /// saying this release is not sold on Qobuz; absence is unknown, not no.
  #[serde(default)]
  pub purchasable: Option<bool>,
  #[serde(default)]
  pub title: String,
  #[serde(default)]
  pub artist: Artist,
  #[serde(default)]
  pub image: ImageSet,
  pub release_date_original: Option<String>,
  /// Date the album becomes available for streaming (ISO YYYY-MM-DD).
  /// When in the future, the album is upcoming and cannot be fetched
  /// via `get_album` yet — Release Watch uses this to gate clicks.
  pub release_date_stream: Option<String>,
  /// Whether the album is currently streamable. False for upcoming
  /// releases, regional restrictions, or label takedowns.
  #[serde(default)]
  pub streamable: Option<bool>,
  pub label: Option<Label>,
  pub genre: Option<Genre>,
  pub tracks_count: Option<u32>,
  pub duration: Option<u32>,
  #[serde(default)]
  pub hires: bool,
  #[serde(default)]
  pub hires_streamable: bool,
  pub maximum_sampling_rate: Option<f64>,
  pub maximum_bit_depth: Option<u32>,
  /// V2 nested quality block. The modern album shape returned by
  /// `/label/getAlbums` (DiscographyAlbumDto) and `/discover`-style items
  /// nests quality here; preferred over the flat `maximum_*` fields.
  #[serde(default)]
  pub audio_info: Option<DiscoverAudioInfo>,
  /// V2 nested release dates (`{original, download, stream}`); preferred
  /// over the flat `release_date_original` when present.
  #[serde(default)]
  pub dates: Option<DiscoverAlbumDates>,
  /// The V2 wire spells the album track count `track_count` (no trailing
  /// `s`); the flat shape uses `tracks_count`.
  #[serde(default)]
  pub track_count: Option<u32>,
  /// Explicit release type when provided ("album" | "ep" | "single" |
  /// "live" | "compilation" | ...).
  #[serde(default)]
  pub release_type: Option<String>,
  /// Alternate release classification on flat album responses.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub product_type: Option<String>,
  #[serde(default)]
  pub tracks: Option<TracksContainer>,
  /// Universal Product Code for the album
  pub upc: Option<String>,
  /// Editorial description/review of the album
  pub description: Option<String>,
  /// Album goodies (booklets, liner notes PDFs, videos).
  ///
  /// LENIENT on purpose. `/album/get` is parsed with the strict structs — its
  /// `Option<T>` fields carry no lenient wrapper, so ONE wrong-typed value
  /// fails the entire album parse and the album simply will not open. That
  /// risk is concentrated exactly here: on albums nobody owns, `goodies` comes
  /// back an empty array, so its populated item shape has never been observed.
  /// The vendor's own downloader gates goodie fetching behind purchases, which
  /// is the hypothesis for why. We cannot capture a populated one without
  /// owning a purchase, so the shape stays UNVERIFIED — and an unverified
  /// shape must degrade to `None`, never take the album down with it.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub goodies: Option<Vec<Goody>>,
  /// Editorial awards (Qobuzissime, Album of the Week, press accolades).
  #[serde(default)]
  pub awards: Option<Vec<AlbumAward>>,
  /// Parental advisory / explicit content marker.
  #[serde(default)]
  pub parental_warning: Option<bool>,
  /// Full artist contributor list including roles. The primary artist is
  /// duplicated here as `roles: ["main-artist"]`; non-main entries are
  /// the album's featured artists.
  #[serde(default)]
  pub artists: Option<Vec<AlbumArtist>>,
  /// Release variant label ("2009 Remaster", "Hi-Res", "Deluxe Edition", …).
  /// Qobuz keeps this out of `title`; the web player appends it in parens so
  /// re-editions of the same album are distinguishable. Surfaced the same way
  /// on every album title (see `format_album_title`).
  #[serde(default)]
  pub version: Option<String>,
  /// Album-level composer credit (single Artist). The official web player
  /// renders this — NOT the per-track `composer` — as the "… • X
  /// (composer)" tail of the header credit line, and suppresses it when the
  /// name is the "Various Composers" placeholder. See `album::build_credits`.
  #[serde(default)]
  pub composer: Option<Artist>,
}

impl Album {
  /// Interpret the catalog's optional availability bit without turning an
  /// omitted field into a withdrawal. Only an explicit `false` means the
  /// album is unavailable.
  pub fn is_streamable(&self) -> bool {
    self.streamable.unwrap_or(true)
  }
}

/// Album artist contributor entry (main artist + featured artists).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumArtist {
  pub id: u64,
  pub name: String,
  #[serde(default)]
  pub roles: Option<Vec<String>>,
}

/// A downloadable extra bundled with an album (e.g. PDF booklet).
///
/// Every field defaults, so a missing key never fails an item, and the whole
/// list is behind `lenient_option` on `Album::goodies`, so a surprising list or
/// item shape degrades to "no goodies" rather than to "the album will not open".
/// Read `url`/`name` through [`Goody::best_url`] / [`Goody::display_name`] rather
/// than directly — the populated shape has never been observed and those helpers
/// are where the fallbacks live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goody {
  #[serde(default)]
  pub id: u64,
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub url: String,
  /// Original (full-size) URL
  #[serde(default)]
  pub original_url: String,
  /// A third spelling seen in vendor payloads; kept as a fallback source for
  /// [`Goody::best_url`] because we cannot confirm which key a real,
  /// purchase-gated goodie uses.
  #[serde(default)]
  pub file_url: Option<String>,
  /// File format id (e.g. 21 for PDF)
  #[serde(default)]
  pub file_format_id: Option<u32>,
  #[serde(default)]
  pub description: Option<String>,
}

impl Goody {
  /// The URL to fetch, trying every spelling we know of. `original_url` wins
  /// because it is documented as the full-size asset; `url` is the common
  /// shape; `file_url` is the unconfirmed third. Returns `None` when the item
  /// carries no usable URL at all, which is the signal to skip it rather than
  /// to fail anything.
  pub fn best_url(&self) -> Option<&str> {
    [self.original_url.as_str(), self.url.as_str(), self.file_url.as_deref().unwrap_or("")]
      .into_iter()
      .map(str::trim)
      .find(|candidate| !candidate.is_empty())
  }

  /// A human label, falling back through the fields most likely to carry one
  /// and finally to the id, so a goodie is never rendered nameless.
  pub fn display_name(&self) -> String {
    for candidate in [Some(self.name.as_str()), self.description.as_deref()].into_iter().flatten() {
      let trimmed = candidate.trim();
      if !trimmed.is_empty() {
        return trimmed.to_string();
      }
    }
    format!("Goody {}", self.id)
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracksContainer {
  pub items: Vec<Track>,
  pub total: u32,
}

/// Artist model
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Artist {
  #[serde(default)]
  pub picture: Option<String>,
  #[serde(default)]
  pub id: u64,
  /// Favourites-list rows only (unix seconds); see `Album::favorited_at`.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub favorited_at: Option<i64>,
  #[serde(default)]
  pub name: String,
  pub image: Option<ImageSet>,
  #[serde(default)]
  pub albums_count: Option<u32>,
  /// Biography (available when fetching full artist details)
  #[serde(default)]
  pub biography: Option<ArtistBiography>,
  /// Albums (available when fetching with extra=albums)
  #[serde(default)]
  pub albums: Option<ArtistAlbums>,
  /// Tracks where this artist appears (extra=tracks_appears_on)
  #[serde(default)]
  pub tracks_appears_on: Option<TracksContainer>,
  /// Curated playlists for this artist (extra=playlists)
  #[serde(default)]
  pub playlists: Option<Vec<Playlist>>,
}

/// Artist biography content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistBiography {
  pub summary: Option<String>,
  pub content: Option<String>,
  pub source: Option<String>,
}

/// Artist albums container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistAlbums {
  pub items: Vec<Album>,
  pub total: u32,
  #[serde(default)]
  pub offset: u32,
  #[serde(default)]
  pub limit: u32,
}

/// Playlist model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
  #[serde(default)]
  pub id: u64,
  #[serde(default)]
  pub name: String,
  pub description: Option<String>,
  #[serde(default)]
  pub owner: PlaylistOwner,
  pub images: Option<Vec<String>>,
  #[serde(default)]
  pub tracks_count: u32,
  #[serde(default)]
  pub duration: u32,
  #[serde(default)]
  pub is_public: bool,
  #[serde(default)]
  pub tracks: Option<TracksContainer>,
  pub genres: Option<Vec<PlaylistGenre>>,
  pub images150: Option<Vec<String>>,
  pub images300: Option<Vec<String>>,
  /// The playlist's OWN Qobuz artwork (editorial playlists). The `images*`
  /// lists above are MEMBER-ALBUM covers, which is why a Qobuz playlist
  /// card binding them shows an album sleeve instead of the playlist
  /// graphic. Absent on user playlists, which fall back to the collage.
  #[serde(default)]
  pub image_rectangle: Option<Vec<String>>,
  #[serde(default)]
  pub image_rectangle_mini: Option<Vec<String>>,
  pub slug: Option<String>,
  pub users_count: Option<u32>,
  /// API revision stamp (unix seconds). The membership index reads it as
  /// change evidence when the API provides it; absent on some list shapes.
  #[serde(default)]
  pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaylistOwner {
  #[serde(default)]
  pub id: u64,
  #[serde(default)]
  pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistGenre {
  pub id: u64,
  pub name: String,
  pub slug: Option<String>,
}

/// Lightweight playlist response with track IDs only
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistWithTrackIds {
  #[serde(default)]
  pub id: u64,
  #[serde(default)]
  pub name: String,
  pub description: Option<String>,
  #[serde(default)]
  pub owner: PlaylistOwner,
  pub images: Option<Vec<String>>,
  #[serde(default)]
  pub tracks_count: u32,
  #[serde(default)]
  pub duration: u32,
  #[serde(default)]
  pub is_public: bool,
  #[serde(default)]
  pub track_ids: Vec<u64>,
  pub genres: Option<Vec<PlaylistGenre>>,
  pub images150: Option<Vec<String>>,
  pub images300: Option<Vec<String>>,
  pub slug: Option<String>,
  pub users_count: Option<u32>,
  /// API revision stamp (unix seconds), when the response carries one.
  #[serde(default)]
  pub updated_at: Option<i64>,
}

/// Result of checking for duplicate tracks in a playlist
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistDuplicateResult {
  pub total_tracks: usize,
  pub duplicate_count: usize,
  pub duplicate_track_ids: HashSet<u64>,
}
