use super::*;
use serde::{Deserialize, Serialize};
// ============ Discover API Types ============

/// Discover index response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverResponse {
  pub containers: DiscoverContainers,
}

/// All discover containers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverContainers {
  pub playlists: Option<DiscoverContainer<DiscoverPlaylist>>,
  pub ideal_discography: Option<DiscoverContainer<DiscoverAlbum>>,
  pub playlists_tags: Option<DiscoverContainer<PlaylistTag>>,
  pub new_releases: Option<DiscoverContainer<DiscoverAlbum>>,
  pub qobuzissims: Option<DiscoverContainer<DiscoverAlbum>>,
  pub most_streamed: Option<DiscoverContainer<DiscoverAlbum>>,
  pub press_awards: Option<DiscoverContainer<DiscoverAlbum>>,
  pub album_of_the_week: Option<DiscoverContainer<DiscoverAlbum>>,
}

/// Generic discover container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverContainer<T> {
  pub id: String,
  pub data: DiscoverData<T>,
}

/// Generic discover data with items
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverData<T> {
  pub has_more: bool,
  pub items: Vec<T>,
}

/// Playlist from discover endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverPlaylist {
  pub id: u64,
  pub name: String,
  pub owner: PlaylistOwner,
  pub image: DiscoverPlaylistImage,
  pub description: Option<String>,
  pub duration: u32,
  pub tracks_count: u32,
  pub genres: Option<Vec<PlaylistGenre>>,
  pub tags: Option<Vec<PlaylistTag>>,
}

/// Playlist image from discover
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverPlaylistImage {
  pub rectangle: Option<String>,
  pub covers: Option<Vec<String>>,
}

/// Playlist tag (for filtering)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTag {
  pub id: u64,
  pub slug: String,
  pub name: String,
}

/// Raw playlist tag from /playlist/getTags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawPlaylistTag {
  pub slug: String,
  pub name_json: String,
  pub position: Option<String>,
  pub is_discover: Option<String>,
  pub featured_tag_id: Option<String>,
}

/// Response from /playlist/getTags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTagsResponse {
  pub tags: Vec<RawPlaylistTag>,
}

/// Response from discover/playlists endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverPlaylistsResponse {
  pub has_more: bool,
  pub items: Vec<DiscoverPlaylist>,
}

/// Album from discover endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverAlbum {
  pub id: String,
  pub title: String,
  pub version: Option<String>,
  pub track_count: Option<u32>,
  pub duration: Option<u32>,
  pub parental_warning: Option<bool>,
  pub image: DiscoverAlbumImage,
  pub artists: Vec<DiscoverArtist>,
  pub label: Option<Label>,
  pub genre: Option<Genre>,
  pub dates: Option<DiscoverAlbumDates>,
  pub audio_info: Option<DiscoverAudioInfo>,
  /// Editorial awards attached to the album. Id 88 = Qobuzissime,
  /// id 151 = Qobuz Album of the Week (locale-stable).
  #[serde(default)]
  pub awards: Option<Vec<AlbumAward>>,
}

/// Album image from discover endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverAlbumImage {
  pub small: Option<String>,
  pub thumbnail: Option<String>,
  pub large: Option<String>,
}

/// Artist in discover album
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverArtist {
  pub id: u64,
  pub name: String,
  pub roles: Option<Vec<String>>,
}

/// Album dates from discover
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverAlbumDates {
  pub download: Option<String>,
  pub original: Option<String>,
  pub stream: Option<String>,
}

/// Audio info from discover album (and, since 2026-08-28, from tracks)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverAudioInfo {
  pub maximum_sampling_rate: Option<f64>,
  pub maximum_bit_depth: Option<u32>,
  pub maximum_channel_count: Option<u32>,
  /// ReplayGain track gain (dB) and peak, as Qobuz ships them on every
  /// track in search/album envelopes
  /// (`qbz-nix-docs/qobuz-api/search-results-response.json`, 15 samples).
  /// Modelled only — no reader yet; the loudness analyser stays as is.
  #[serde(default)]
  pub replaygain_track_gain: Option<f64>,
  #[serde(default)]
  pub replaygain_track_peak: Option<f64>,
}

// ============ Artist Page Types (/artist/page) ============

/// Top-level response from /artist/page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistResponse {
  pub id: u64,
  pub name: PageArtistName,
  pub artist_category: Option<String>,
  pub biography: Option<PageArtistBiography>,
  pub images: Option<PageArtistImages>,
  pub similar_artists: Option<PageArtistSimilar>,
  pub top_tracks: Option<Vec<PageArtistTrack>>,
  pub last_release: Option<PageArtistRelease>,
  pub releases: Option<Vec<PageArtistReleaseGroup>>,
  pub tracks_appears_on: Option<Vec<PageArtistTrack>>,
  pub playlists: Option<PageArtistPlaylists>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistName {
  pub display: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistBiography {
  pub content: Option<String>,
  pub source: Option<serde_json::Value>,
  pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistImages {
  pub portrait: Option<PageArtistPortrait>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistPortrait {
  #[serde(default)]
  pub hash: String,
  #[serde(default)]
  pub format: String,
  #[serde(flatten)]
  pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistSimilar {
  pub has_more: bool,
  pub items: Vec<PageArtistSimilarItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistSimilarItem {
  pub id: u64,
  pub name: PageArtistName,
  pub images: Option<PageArtistImages>,
}

/// A group of releases by type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistReleaseGroup {
  #[serde(rename = "type")]
  pub release_type: String,
  pub has_more: bool,
  pub items: Vec<PageArtistRelease>,
}

/// A release item from /artist/page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistRelease {
  pub id: String,
  pub title: String,
  pub version: Option<String>,
  pub tracks_count: Option<u32>,
  pub artist: Option<PageArtistReleaseArtist>,
  pub artists: Option<Vec<PageArtistReleaseContributor>>,
  pub image: Option<ImageSet>,
  pub label: Option<Label>,
  pub genre: Option<Genre>,
  pub release_type: Option<String>,
  pub release_tags: Option<Vec<String>>,
  pub duration: Option<u32>,
  pub dates: Option<DiscoverAlbumDates>,
  pub parental_warning: Option<bool>,
  pub audio_info: Option<DiscoverAudioInfo>,
  pub rights: Option<PageArtistRights>,
  pub awards: Option<Vec<PageArtistAward>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistReleaseArtist {
  pub id: u64,
  pub name: PageArtistName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistReleaseContributor {
  pub id: u64,
  pub name: String,
  pub roles: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistRights {
  pub streamable: Option<bool>,
  pub hires_streamable: Option<bool>,
  pub hires_purchasable: Option<bool>,
  pub purchasable: Option<bool>,
  pub downloadable: Option<bool>,
  pub previewable: Option<bool>,
  pub sampleable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistAward {
  pub id: u64,
  pub name: String,
  pub awarded_at: Option<String>,
}

/// Award attached to an album. Shape is intentionally lenient because
/// Qobuz uses three different embedded shapes across endpoints:
/// - `/discover/index` — {id: int, name, awarded_at: "YYYY-MM-DD"}
/// - `/album/get`      — LegacyAwardDto {awardId: string, name,
///                        publicationId, publicationName, awardSlug,
///                        awardedAt: long, …}
/// - `/artist/page`    — PageArtistAward {id: int, name, awarded_at}
/// id is emitted as String downstream so the frontend has a single
/// type to carry into /award/page and /award/getAlbums. The `alias`
/// list covers the LegacyAwardDto field name the web app never sees
/// but the mobile API uses.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlbumAward {
  #[serde(
    default,
    alias = "awardId",
    alias = "award_id",
    deserialize_with = "deserialize_award_id"
  )]
  pub id: Option<String>,
  #[serde(default)]
  pub name: String,
  #[serde(default, alias = "awardedAt", deserialize_with = "deserialize_award_awarded_at")]
  pub awarded_at: Option<String>,
}

fn deserialize_award_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let value = Option::<serde_json::Value>::deserialize(deserializer)?;
  Ok(match value {
    Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
    Some(serde_json::Value::Number(n)) => Some(n.to_string()),
    _ => None,
  })
}

fn deserialize_award_awarded_at<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let value = Option::<serde_json::Value>::deserialize(deserializer)?;
  Ok(match value {
    Some(serde_json::Value::String(s)) => Some(s),
    Some(serde_json::Value::Number(n)) => Some(n.to_string()),
    _ => None,
  })
}

/// Track from /artist/page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistTrack {
  pub id: u64,
  pub title: String,
  pub version: Option<String>,
  pub duration: Option<u32>,
  pub isrc: Option<String>,
  pub parental_warning: Option<bool>,
  pub artist: Option<PageArtistReleaseArtist>,
  pub composer: Option<serde_json::Value>,
  pub audio_info: Option<DiscoverAudioInfo>,
  pub rights: Option<PageArtistRights>,
  pub physical_support: Option<PageArtistPhysicalSupport>,
  pub album: Option<PageArtistTrackAlbum>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistPhysicalSupport {
  pub media_number: Option<u32>,
  pub track_number: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistTrackAlbum {
  pub id: String,
  pub title: String,
  pub version: Option<String>,
  pub image: Option<ImageSet>,
  pub label: Option<Label>,
  pub genre: Option<Genre>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistPlaylists {
  pub has_more: bool,
  pub items: Vec<PageArtistPlaylist>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistPlaylist {
  pub id: u64,
  pub title: Option<String>,
  pub description: Option<String>,
  pub owner: Option<PageArtistPlaylistOwner>,
  pub tracks_count: Option<u32>,
  pub duration: Option<u32>,
  pub images: Option<PageArtistPlaylistImages>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistPlaylistOwner {
  pub id: u64,
  pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageArtistPlaylistImages {
  pub rectangle: Option<Vec<String>>,
}

/// Response from /artist/getReleasesGrid
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasesGridResponse {
  pub has_more: bool,
  pub items: Vec<PageArtistRelease>,
}

// ============ Artist Story Types (/artist/story) ============

/// Response from /artist/story (Magazine / editorial articles about the artist).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistStoryResponse {
  pub has_more: bool,
  #[serde(default)]
  pub items: Vec<ArtistStoryItem>,
}

/// A single Magazine story. `image`/`images[].url` are ready-to-use signed
/// arc-cdn URLs — do NOT run them through the portrait hash/format builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistStoryItem {
  pub id: String,
  pub title: String,
  /// Epoch SECONDS, not an ISO string.
  #[serde(default)]
  pub display_date: Option<i64>,
  #[serde(default)]
  pub image: Option<String>,
  #[serde(default)]
  pub images: Option<Vec<ArtistStoryImage>>,
  #[serde(default)]
  pub description_short: Option<String>,
  #[serde(default)]
  pub authors: Option<Vec<ArtistStoryAuthor>>,
  #[serde(default)]
  pub section_slugs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistStoryImage {
  #[serde(default)]
  pub format: Option<String>,
  pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistStoryAuthor {
  pub name: String,
  #[serde(default)]
  pub id: Option<String>,
  #[serde(default)]
  pub slug: Option<String>,
}
