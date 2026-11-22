use super::*;
use serde::{Deserialize, Serialize};
// ============ Metadata Types ============

/// Label model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
  pub id: u64,
  pub name: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub image: Option<serde_json::Value>,
}

// ============ Label Page Types (/label/page) ============

/// Top-level response from /label/page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelPageData {
  pub id: u64,
  pub name: String,
  pub description: Option<String>,
  #[serde(default)]
  pub image: Option<serde_json::Value>,
  #[serde(default)]
  pub releases: Option<Vec<LabelPageContainer>>,
  #[serde(default)]
  pub playlists: Option<LabelPageGenericList>,
  #[serde(default)]
  pub top_tracks: Option<Vec<serde_json::Value>>,
  #[serde(default)]
  pub top_artists: Option<LabelPageGenericList>,
}

/// A container within label page (e.g. releases category)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelPageContainer {
  pub id: Option<String>,
  pub data: Option<LabelPageGenericList>,
}

/// Generic list with has_more and items
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelPageGenericList {
  pub has_more: Option<bool>,
  pub items: Option<Vec<serde_json::Value>>,
}

// ============ Award Page Types (/award/page) ============

/// Magazine/publisher behind a press award.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardMagazine {
  #[serde(default, deserialize_with = "deserialize_string_or_int")]
  pub id: Option<String>,
  #[serde(default)]
  pub name: Option<String>,
  #[serde(default)]
  pub image: Option<String>,
}

/// Top-level response from /award/page. Fields all Optional because
/// Android's AwardDto marks everything nullable and Qobuz is loose
/// about which ones come back on any given request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardPageData {
  #[serde(default, deserialize_with = "deserialize_string_or_int")]
  pub id: Option<String>,
  #[serde(default)]
  pub name: Option<String>,
  #[serde(default)]
  pub image: Option<String>,
  #[serde(default, deserialize_with = "deserialize_string_or_int")]
  pub awarded_at: Option<String>,
  #[serde(default)]
  pub magazine: Option<AwardMagazine>,
  /// Categorized containers of award-winning releases (matches
  /// Android's `releases: List<GenericContainerDto<AlbumDto>>`).
  #[serde(default)]
  pub releases: Option<Vec<AwardPageContainer>>,
  #[serde(default)]
  pub playlists: Option<AwardPageGenericList>,
}

fn deserialize_string_or_int<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardPageContainer {
  pub id: Option<String>,
  pub data: Option<AwardPageGenericList>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardPageGenericList {
  pub has_more: Option<bool>,
  pub items: Option<Vec<serde_json::Value>>,
}

/// Response from /label/explore
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelExploreResponse {
  pub has_more: Option<bool>,
  pub items: Option<Vec<serde_json::Value>>,
}

// ============ Label Sub-resource Types (v9.7.0.3 API) ============
//
// The label page (/label/page) returns an aggregated snapshot; the
// getAlbums / getPlaylists / getTopArtists / getNextReleases /
// getAwardedReleases endpoints return paginated lists for each
// sub-resource. Per Qobuz convention these use the V2 list envelope
// { has_more, items: [...] }. Deserialized shapes are best-effort: if
// the server wraps items in e.g. { albums: { items: ... } }, the
// Optional fallbacks still keep the call non-fatal.

/// Generic paginated response from /label/get* endpoints.
///
/// `T` is typed (`Album`, `Playlist`, `Artist`) but all fields are
/// tolerant so the same struct works if the server returns a bare
/// items list or a nested one.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabelListPage<T> {
  #[serde(default)]
  pub has_more: Option<bool>,
  #[serde(default = "Vec::new")]
  pub items: Vec<T>,
  #[serde(default)]
  pub total: Option<u32>,
  #[serde(default)]
  pub offset: Option<u32>,
  #[serde(default)]
  pub limit: Option<u32>,
}

/// Response from /label/story.
///
/// Shape inferred from `c30/b.java` — returns editorial / story content
/// for a label. Actual fields beyond the label identity are not fully
/// known; everything past `id` / `name` / `description` is kept open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelStoryResponse {
  pub id: Option<u64>,
  pub name: Option<String>,
  #[serde(default)]
  pub description: Option<String>,
  #[serde(default)]
  pub story: Option<String>,
  #[serde(default)]
  pub image: Option<serde_json::Value>,
  #[serde(default)]
  pub has_more: Option<bool>,
  #[serde(default)]
  pub items: Option<Vec<serde_json::Value>>,
}

/// Response from /label/getList (POST). Bulk lookup that hydrates
/// label metadata for a set of label IDs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabelGetListResponse {
  #[serde(default, deserialize_with = "deserialize_label_list")]
  pub labels: Vec<Label>,
  /// Fallback for unknown envelope shape — preserved as raw JSON if
  /// the server wraps differently than expected.
  #[serde(default)]
  pub extra: Option<serde_json::Value>,
}

fn deserialize_label_list<'de, D>(deserializer: D) -> Result<Vec<Label>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let mut value = serde_json::Value::deserialize(deserializer)?;
  if value.is_object() {
    value = value
      .get_mut("items")
      .map(serde_json::Value::take)
      .ok_or_else(|| serde::de::Error::custom("The label list has no items"))?;
  }
  serde_json::from_value(value).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod label_tests {
  use super::*;
  use serde_json::{json, Value};

  #[test]
  fn bulk_labels_keep_artwork_in_both_wire_envelopes() {
    let labels = json!([{"id":1,"name":"A label","image":"https://static.qobuz.com/label.jpg"}]);
    for value in [json!({"labels":labels.clone()}), json!({"labels":{"total":1,"items":labels}})] {
      let page: LabelGetListResponse = serde_json::from_value(value).unwrap();
      assert_eq!(page.labels.len(), 1);
      assert_eq!(
        page.labels[0].image.as_ref().and_then(Value::as_str),
        Some("https://static.qobuz.com/label.jpg")
      );
    }
  }

  #[test]
  fn malformed_bulk_envelopes_are_not_successful_empty_pages() {
    assert!(serde_json::from_value::<LabelGetListResponse>(json!({"labels":{"error":"invalid"}}))
      .is_err());
  }
}

/// Genre model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Genre {
  pub id: u64,
  pub name: String,
  /// Full ancestor id chain (top-level first, self last) as sent by the
  /// discover endpoints. Absent on older cached payloads → None.
  #[serde(default)]
  pub path: Option<Vec<u64>>,
}

/// Genre info with full details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreInfo {
  pub id: u64,
  pub name: String,
  #[serde(default)]
  pub color: Option<String>,
  #[serde(default)]
  pub slug: Option<String>,
  #[serde(default)]
  pub path: Option<Vec<u64>>,
}

/// Genre list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreListResponse {
  pub genres: GenreListContainer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreListContainer {
  pub items: Vec<GenreInfo>,
}

// ============ Search Types ============

/// Search results container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
  pub albums: Option<SearchResultsPage<Album>>,
  pub tracks: Option<SearchResultsPage<Track>>,
  pub artists: Option<SearchResultsPage<Artist>>,
  pub playlists: Option<SearchResultsPage<Playlist>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResultsPage<T> {
  #[serde(default = "Vec::new")]
  pub items: Vec<T>,
  // `/album/suggest` returns a page with only `{limit, items}` (no `total`
  // or `offset`); without defaults the whole response failed to deserialize
  // and the album "Suggestions" carousel silently never showed. Defaulting
  // the pagination scalars to 0 is harmless — only `items` is consumed there.
  #[serde(default)]
  pub total: u32,
  #[serde(default)]
  pub offset: u32,
  #[serde(default)]
  pub limit: u32,
}

// ============ Purchases API Models ============
//
// Ported field-for-field from `src-tauri/src/api/models.rs:546-628`. These are
// the wire shapes returned by the Qobuz `/purchase/*` endpoints. The lenient
// deserializers live in `crate::purchase_serde` (see that module's docs for the
// per-field coercion rules).

/// Response from `/purchase/getUserPurchases` (commands #1/#3/#4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurchaseResponse {
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_page")]
  pub albums: SearchResultsPage<PurchaseAlbum>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_page")]
  pub tracks: SearchResultsPage<PurchaseTrack>,
}

/// Response from `/purchase/getUserPurchasesIds` (command #2). Items are OPAQUE
/// JSON — the UI reads only `.total` from each page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurchaseIdsResponse {
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_page")]
  pub albums: SearchResultsPage<serde_json::Value>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_page")]
  pub tracks: SearchResultsPage<serde_json::Value>,
}

/// A purchased album. `downloadable` defaults TRUE; `downloaded` is NOT from
/// Qobuz — it is server-computed from the local registry. `purchased_at` is
/// unix epoch seconds. Nested `tracks` is populated only on the album-detail /
/// by-type-albums paths.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PurchaseAlbum {
  #[serde(default, deserialize_with = "crate::purchase_serde::deserialize_string_id")]
  pub id: String,
  #[serde(default)]
  pub title: String,
  #[serde(default)]
  pub artist: Artist,
  #[serde(default)]
  pub image: ImageSet,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub release_date_original: Option<String>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub label: Option<Label>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub genre: Option<Genre>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub tracks_count: Option<u32>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub duration: Option<u32>,
  #[serde(default)]
  pub hires: bool,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub maximum_sampling_rate: Option<f64>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub maximum_bit_depth: Option<u32>,
  #[serde(default = "crate::purchase_serde::serde_true")]
  pub downloadable: bool,
  /// THE ENTITLEMENT: the `format_id`s this purchase may be downloaded in.
  /// First seen on a real account 2026-09-01 — `[55]` (DSD64), `[56]`
  /// (DSD128), `[7, 6, 5]` (a hi-res FLAC purchase) — and it is the only
  /// authority for the download menu: a DSD purchase reports `hires:false`
  /// with a 16/44.1 streaming catalog, so nothing can be inferred from the
  /// album's own quality fields. Empty on older captures.
  #[serde(default)]
  pub downloadable_format_ids: Vec<u32>,
  /// Purchased AS hi-res, independent of what the catalog streams
  /// (`hires:false, hires_purchased:true` on the DSD64 album).
  #[serde(default)]
  pub hires_purchased: bool,
  #[serde(default)]
  pub downloaded: bool,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub purchased_at: Option<i64>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub tracks: Option<SearchResultsPage<PurchaseTrack>>,
}

/// A purchased track.
///
/// It DOES carry a `version` (the track subtitle/edition). It deliberately did
/// not, on the reasoning that the purchases endpoints never send one — which is
/// true of the endpoints and beside the point for the screen: the album-detail
/// view builds its tracks from the CATALOG album (`/album/get`), which does send
/// `version`, and the reference's frontend renders
/// `formatTrackTitle(title, version)`. Dropping the field is what made every
/// purchased track lose its subtitle (issue #360); contract §10-C rules the
/// field back in and `build_purchase_album` maps it across.
///
/// `streamable` defaults TRUE; `downloaded`/`downloaded_format_ids` are
/// server-computed from the local registry; `media_number` is the disc number
/// used for disc-grouping. `purchased_at` is unix epoch seconds.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PurchaseTrack {
  #[serde(default, deserialize_with = "crate::purchase_serde::deserialize_u64_id")]
  pub id: u64,
  #[serde(default)]
  pub title: String,
  /// Track subtitle ("Remastered", "Live at …"). The purchases endpoints do
  /// not send it, but the catalog `/album/get` track does, and the detail
  /// screen renders `formatTrackTitle(title, version)` — so `build_purchase_album`
  /// carries it across. Tauri declared the same field on its frontend type and
  /// never mapped it, which is why purchased track titles lost their version
  /// (issue #360); this field is what closes that.
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub version: Option<String>,
  #[serde(default)]
  pub track_number: u32,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub media_number: Option<u32>,
  #[serde(default)]
  pub duration: u32,
  #[serde(default)]
  pub performer: Artist,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub album: Option<AlbumSummary>,
  #[serde(default)]
  pub hires: bool,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub maximum_sampling_rate: Option<f64>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub maximum_bit_depth: Option<u32>,
  #[serde(default = "crate::purchase_serde::serde_true")]
  pub streamable: bool,
  #[serde(default)]
  pub downloaded: bool,
  #[serde(default)]
  pub downloaded_format_ids: Vec<u32>,
  /// The per-track entitlement, same meaning as the album's. Not yet observed
  /// on a real `type=tracks` page (every capture so far had zero standalone
  /// track purchases); defaulted so an absent key costs nothing.
  #[serde(default)]
  pub downloadable_format_ids: Vec<u32>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_option")]
  pub purchased_at: Option<i64>,
}

/// A downloadable format option synthesized client-side from an `Album`
/// (command #6). `id` feeds `getFileUrl`'s `format_id`; `label` (with `/`→`-`)
/// becomes the `qualityDir` subfolder. The synthesis table lives in the
/// orchestration service (Slice 4); this struct is just the wire/UI shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurchaseFormatOption {
  pub id: u32,
  pub label: String,
  pub bit_depth: Option<u32>,
  pub sampling_rate: Option<f64>,
  /// `false` = the purchased file, fetched through the purchase grant
  /// (`getFileUrl`, `intent=download`), which the store only issues for the
  /// ids in the entitlement. `true` = a lower quality the store does NOT
  /// sell for this purchase, fetched through the streaming grant
  /// (`intent=stream`) instead — the route the Slint/Tauri builds used for
  /// every download, kept only BELOW the purchased quality.
  #[serde(default)]
  pub streaming: bool,
}

/// Response from `/album/suggest` — albums similar to a seed album.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumSuggestResponse {
  #[serde(default)]
  pub algorithm: Option<String>,
  #[serde(default)]
  pub albums: Option<SearchResultsPage<Album>>,
}

/// Response from the `/radio/*` endpoints — a generated track list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadioResponse {
  #[serde(rename = "type", default)]
  pub radio_type: Option<String>,
  #[serde(default)]
  pub title: Option<String>,
  #[serde(default, deserialize_with = "crate::purchase_serde::lenient_page_flexible")]
  pub tracks: SearchResultsPage<Track>,
}

/// One entry of the Qobuz `most_popular` block in a combined search.
/// Serde tagging matches the legacy `V2MostPopularItem` so the Tauri
/// command's response shape is unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content", rename_all = "lowercase")]
pub enum MostPopularItem {
  Tracks(Track),
  Albums(Album),
  Artists(Artist),
}

/// Combined search result: the four category pages plus an optional
/// "most popular" hero entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchAllResults {
  pub albums: SearchResultsPage<Album>,
  pub tracks: SearchResultsPage<Track>,
  pub artists: SearchResultsPage<Artist>,
  pub playlists: SearchResultsPage<Playlist>,
  pub most_popular: Option<MostPopularItem>,
}

/// Favorites container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Favorites {
  pub albums: Option<SearchResultsPage<Album>>,
  pub tracks: Option<SearchResultsPage<Track>>,
  pub artists: Option<SearchResultsPage<Artist>>,
}
