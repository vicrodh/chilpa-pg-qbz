use crate::projection;
use qobuz_api::{library_pages::LibraryPage, ApiError, QobuzClient, Result};
use serde_json::{json, Map, Value};
use std::{
  sync::Mutex,
  time::{Duration, Instant},
};

pub(crate) const LIBRARY_VIEWS: [(&str, &str, &str); 7] = [
  ("all", "All", "Todo"),
  ("playlists", "Playlists", "Playlists"),
  ("tracks", "Tracks", "Pistas"),
  ("albums", "Releases", "Álbumes"),
  ("artists", "Artists", "Artistas"),
  ("labels", "Labels", "Sellos"),
  ("awards", "Awards", "Premios"),
];

pub(crate) fn library_title(view: &str, spanish: bool) -> &'static str {
  LIBRARY_VIEWS
    .iter()
    .find(|(id, _, _)| *id == view)
    .map(|(_, en, es)| if spanish { *es } else { *en })
    .unwrap_or(if spanish { "Biblioteca" } else { "Library" })
}

pub(crate) fn filter_controls(
  view: &str,
  spanish: bool,
  purchases_only: bool,
  genres: &[Value],
) -> Vec<Value> {
  let label = |en: &'static str, es: &'static str| if spanish { es } else { en };
  let mut controls = vec![json!({"key":"text","type":"search","label":label("Search","Buscar")})];
  let mut memberships = vec![json!({"value":"all","label":label("All","Todo")})];
  if matches!(view, "all" | "playlists" | "albums" | "tracks") {
    memberships.push(json!({"value":"favorites","label":label("Favorites only","Solo favoritos")}));
  }
  if matches!(view, "all" | "albums" | "tracks") {
    memberships.push(json!({"value":"purchases","label":label("Purchases only","Solo compras"),"excludes":["hide_purchases"]}));
  }
  if matches!(view, "all" | "artists" | "labels" | "awards") {
    memberships.push(json!({"value":"followed","label":if view == "awards" {
      label("Favorites / Following","Favoritos / Seguidos")
    } else { label("Following only","Solo seguidos") }}));
  }
  if memberships.len() > 1 {
    controls.push(json!({"key":"membership","type":"choice","label":label("Collection","Colección"),"options":memberships}));
  }
  if matches!(view, "all" | "albums" | "tracks") {
    let mut options = vec![json!({"value":"","label":label("All genres","Todos los géneros")})];
    options.extend(genres.iter().take(255).cloned());
    controls.push(
      json!({"key":"genre","type":"choice","label":label("Genre","Género"),"options":options}),
    );
    if !purchases_only {
      controls.push(json!({"key":"hide_purchases","type":"toggle","label":label("Hide purchases","Ocultar compras")}));
    }
    controls
      .push(json!({"key":"hires","type":"toggle","label":label("Hi-Res only","Solo Hi-Res")}));
  }
  if view == "playlists" {
    controls.push(json!({"key":"owner","type":"choice","label":label("Owner","Propietario"),"options":[
      {"value":"all","label":label("All","Todos")},{"value":"me","label":label("My playlists","Mis playlists")},
      {"value":"qobuz","label":"Qobuz"},{"value":"other","label":label("Other listeners","Otros oyentes")},
    ]}));
  }
  controls
}

struct GenreOptions {
  next_check: Instant,
  options: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct LibraryGenres {
  // Two locale entries, at most 255 names each, refreshed on demand after one day.
  entries: Mutex<[Option<GenreOptions>; 2]>,
  fetching: tokio::sync::Mutex<()>,
}

impl LibraryGenres {
  pub async fn options(&self, api: &QobuzClient, spanish: bool, online: bool) -> Vec<Value> {
    let position = usize::from(spanish);
    let cached = {
      let entries = self.entries.lock().unwrap_or_else(|poison| poison.into_inner());
      let entry = entries[position].as_ref();
      if entry.is_some_and(|entry| entry.next_check > Instant::now()) || !online {
        return entry.map(|entry| entry.options.clone()).unwrap_or_default();
      }
      entry.map(|entry| entry.options.clone()).unwrap_or_default()
    };
    let Ok(_fetching) = self.fetching.try_lock() else {
      return cached;
    };
    let result = tokio::time::timeout(Duration::from_secs(10), api.get_genres(None)).await;
    let (options, lifetime) = match result {
      Ok(Ok(genres)) => {
        let options = genres
          .into_iter()
          .take(255)
          .filter_map(|genre| {
            let name = genre.name.trim();
            (genre.id > 0 && !name.is_empty() && name.len() <= 128)
              .then(|| json!({"value":genre.id.to_string(),"label":name}))
          })
          .collect();
        (options, Duration::from_secs(86400))
      }
      _ => (cached, Duration::from_secs(600)),
    };
    self.entries.lock().unwrap_or_else(|poison| poison.into_inner())[position] =
      Some(GenreOptions { next_check: Instant::now() + lifetime, options: options.clone() });
    options
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibrarySource {
  FavoriteAlbums,
  FavoriteTracks,
  FollowedArtists,
  FollowedLabels,
  FollowedAwards,
  PurchasedAlbums,
  PurchasedTracks,
  Playlists,
}

pub(crate) struct SourceRow {
  pub card: Value,
  pub added_at: i64,
  pub explicit: bool,
}

pub(crate) struct SourcePage {
  pub items: Vec<SourceRow>,
  pub next_offset: Option<u64>,
  pub total: Option<u64>,
}

impl LibrarySource {
  pub const ALL: [Self; 8] = [
    Self::FavoriteAlbums,
    Self::FavoriteTracks,
    Self::FollowedArtists,
    Self::FollowedLabels,
    Self::FollowedAwards,
    Self::PurchasedAlbums,
    Self::PurchasedTracks,
    Self::Playlists,
  ];

  pub fn name(self) -> &'static str {
    match self {
      Self::FavoriteAlbums => "favorite-albums",
      Self::FavoriteTracks => "favorite-tracks",
      Self::FollowedArtists => "followed-artists",
      Self::FollowedLabels => "followed-labels",
      Self::FollowedAwards => "followed-awards",
      Self::PurchasedAlbums => "purchased-albums",
      Self::PurchasedTracks => "purchased-tracks",
      Self::Playlists => "playlists",
    }
  }

  pub fn kind(self) -> &'static str {
    match self {
      Self::FavoriteAlbums | Self::PurchasedAlbums => "album",
      Self::FavoriteTracks | Self::PurchasedTracks => "track",
      Self::FollowedArtists => "artist",
      Self::FollowedLabels => "label",
      Self::FollowedAwards => "award",
      Self::Playlists => "playlist",
    }
  }

  pub fn membership(self) -> &'static str {
    match self {
      Self::FavoriteAlbums | Self::FavoriteTracks | Self::Playlists => "favorite",
      Self::FollowedArtists | Self::FollowedLabels | Self::FollowedAwards => "followed",
      Self::PurchasedAlbums | Self::PurchasedTracks => "purchased",
    }
  }

  pub async fn fetch_page(
    self,
    api: &QobuzClient,
    user_id: u64,
    offset: u64,
    limit: u32,
  ) -> Result<SourcePage> {
    let page = match self {
      Self::FavoriteAlbums => api.library_favorites_page("albums", limit, offset).await?,
      Self::FavoriteTracks => api.library_favorites_page("tracks", limit, offset).await?,
      Self::FollowedArtists => api.library_favorites_page("artists", limit, offset).await?,
      Self::FollowedLabels => api.library_favorites_page("labels", limit, offset).await?,
      Self::FollowedAwards => api.library_favorites_page("awards", limit, offset).await?,
      Self::PurchasedAlbums => api.library_purchases_page("albums", limit, offset).await?,
      Self::PurchasedTracks => api.library_purchases_page("tracks", limit, offset).await?,
      Self::Playlists => api.library_playlists_page(limit, offset).await?,
    };
    self.project_page(page, user_id)
  }

  fn project_page(self, page: LibraryPage, user_id: u64) -> Result<SourcePage> {
    let items =
      page.items.iter().map(|raw| self.project_row(raw, user_id)).collect::<Result<_>>()?;
    Ok(SourcePage { items, next_offset: page.next_offset, total: page.total })
  }

  fn project_row(self, raw: &Value, user_id: u64) -> Result<SourceRow> {
    let dates: &[&str] = match self {
      Self::PurchasedAlbums | Self::PurchasedTracks => &["purchased_at"],
      Self::Playlists => &["subscribed_at", "created_at"],
      _ => &["favorited_at"],
    };
    let added_at = timestamp(raw, dates)?;
    let mut card = thin_card(raw, self.kind(), user_id)?;
    card["added_at"] = json!(added_at);
    card[self.membership()] = json!(true);
    if matches!(self.kind(), "artist" | "label" | "award") {
      card["favorite"] = json!(self.membership() == "followed");
    }
    Ok(SourceRow { card, added_at, explicit: true })
  }
}

fn invalid(message: &str) -> ApiError {
  ApiError::ApiResponse(format!("Invalid library item: {message}"))
}

fn text<'a>(raw: &'a Value, paths: &[&str]) -> Option<&'a str> {
  paths.iter().find_map(|path| raw.pointer(path)?.as_str().map(str::trim).filter(|s| !s.is_empty()))
}

fn identifier(value: Option<&Value>) -> Option<String> {
  match value? {
    Value::String(text)
      if !text.is_empty()
        && text.len() <= 256
        && text.parse::<u64>().ok() != Some(0)
        && !text.chars().any(|c| c.is_whitespace() || c == ':' || c.is_control()) =>
    {
      Some(text.clone())
    }
    Value::Number(number) if number.as_u64().is_some_and(|id| id != 0) => Some(number.to_string()),
    _ => None,
  }
}

fn put_text(card: &mut Map<String, Value>, field: &str, value: Option<&str>) -> Result<()> {
  if let Some(value) = value {
    if value.len() > 4096 {
      return Err(invalid("display text exceeds 4096 bytes"));
    }
    card.insert(field.to_owned(), json!(value));
  }
  Ok(())
}

fn timestamp(raw: &Value, keys: &[&str]) -> Result<i64> {
  let Some(value) = keys.iter().find_map(|key| raw.get(key).filter(|value| !value.is_null()))
  else {
    return Ok(0);
  };
  if let Some(seconds) = value.as_i64().filter(|seconds| *seconds >= 0) {
    return Ok(seconds);
  }
  if let Some(seconds) = value
    .as_f64()
    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0 && *seconds < i64::MAX as f64)
  {
    return Ok(seconds.floor() as i64);
  }
  if let Some(text) = value.as_str() {
    if let Ok(seconds) = text.parse::<i64>() {
      return (seconds >= 0)
        .then_some(seconds)
        .ok_or_else(|| invalid("negative membership timestamp"));
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
      return Ok(date.timestamp());
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
      return Ok(date.and_hms_opt(0, 0, 0).expect("midnight").and_utc().timestamp());
    }
  }
  Err(invalid("invalid membership timestamp"))
}

pub(crate) fn thin_card(raw: &Value, kind: &str, user_id: u64) -> Result<Value> {
  let id = identifier(raw.get("id")).ok_or_else(|| invalid("missing identity"))?;
  let title = if matches!(kind, "album" | "track") {
    text(raw, &["/title"])
  } else {
    text(raw, &["/name/display", "/name", "/title"])
  }
  .ok_or_else(|| invalid("missing title"))?;
  let mut card = Map::new();
  card.insert("kind".into(), json!(kind));
  card.insert("ref".into(), json!(format!("{kind}:{id}")));
  put_text(&mut card, "title", Some(title))?;
  if let Some(version) = text(raw, &["/version"]).filter(|version| !title.contains(version)) {
    put_text(&mut card, "title", Some(&format!("{title} ({version})")))?;
  }
  let album = raw.get("album").filter(|album| album.is_object());
  let metadata = if kind == "track" { album.unwrap_or(raw) } else { raw };
  if kind == "track" {
    if let Some(artist) = qobuz_models::artist_display::track_artist(raw, album) {
      put_text(&mut card, "artist", Some(&artist.name))?;
      if let Some(id) = artist.id {
        card.insert("artist_ref".into(), json!(format!("artist:{id}")));
      }
    }
    put_text(&mut card, "album", text(raw, &["/album/title"]))?;
    if let Some(id) = identifier(raw.pointer("/album/id")) {
      card.insert("album_ref".into(), json!(format!("album:{id}")));
    }
  } else if kind == "album" {
    put_text(
      &mut card,
      "artist",
      text(
        raw,
        &["/artist/name/display", "/artist/name", "/artists/0/name/display", "/artists/0/name"],
      ),
    )?;
    if let Some(id) = identifier(raw.pointer("/artist/id").or_else(|| raw.pointer("/artists/0/id")))
    {
      card.insert("artist_ref".into(), json!(format!("artist:{id}")));
    }
  }
  if let Some(image) = projection::image(metadata)
    .or_else(|| projection::image(raw))
    .or_else(|| (kind == "award").then(|| projection::image(&raw["magazine"])).flatten())
  {
    put_text(&mut card, "image", Some(&image))?;
    if kind == "playlist" {
      card.insert("art_fit".into(), json!("contain"));
    }
  }
  if matches!(kind, "label" | "award") {
    card.insert("art_fit".into(), json!("contain"));
  }
  if kind == "award" {
    put_text(&mut card, "subtitle", text(raw, &["/magazine/name"]))?;
  }
  put_text(&mut card, "label", text(metadata, &["/label/name"]))?;
  if let Some(id) = identifier(metadata.pointer("/label/id")) {
    card.insert("label_ref".into(), json!(format!("label:{id}")));
  }
  if let Some(image) = metadata.get("label").and_then(projection::image) {
    put_text(&mut card, "label_image", Some(&image))?;
  }
  for artist in raw
    .get("performer")
    .into_iter()
    .chain(raw.get("artist"))
    .chain(raw.get("artists").and_then(Value::as_array).into_iter().flatten())
    .chain(metadata.get("artist"))
    .chain(metadata.get("artists").and_then(Value::as_array).into_iter().flatten())
  {
    if let Some(id) = identifier(artist.get("id")) {
      if card
        .get("artist_ref")
        .and_then(Value::as_str)
        .is_some_and(|reference| reference.strip_prefix("artist:") == Some(id.as_str()))
      {
        if let Some(image) = projection::image(artist) {
          put_text(&mut card, "artist_image", Some(&image))?;
          break;
        }
      }
    }
  }
  if matches!(kind, "album" | "track") {
    let mut artists: Vec<Value> = Vec::new();
    for artist in raw
      .get("interpreters")
      .filter(|_| kind == "track")
      .and_then(Value::as_array)
      .filter(|artists| !artists.is_empty())
      .or_else(|| raw.get("artists").and_then(Value::as_array))
      .into_iter()
      .flatten()
    {
      if artist.get("roles").and_then(Value::as_array).is_some_and(|roles| {
        !roles.is_empty()
          && !roles.iter().filter_map(Value::as_str).any(|role| {
            matches!(
              role.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str(),
              "mainartist" | "performer" | "featuredartist" | "interpreter"
            )
          })
      }) {
        continue;
      }
      let Some(id) = identifier(artist.get("id")) else {
        continue;
      };
      let Some(name) = text(artist, &["/name/display", "/name"]) else {
        continue;
      };
      let reference = format!("artist:{id}");
      if artists.iter().any(|artist| artist["ref"] == reference) {
        continue;
      }
      if artists.len() >= 32 {
        break;
      }
      if name.len() > 512 {
        continue;
      }
      let image = projection::image(artist).filter(|image| image.len() <= 1024);
      artists.push(json!({"ref":reference,"title":name,"image":image}));
    }
    if !artists.is_empty() {
      card.insert("library_artists".into(), json!(artists));
    }
  }
  if let Some(date) = text(
    metadata,
    &["/release_date_original", "/dates/original", "/release_date_stream", "/dates/stream"],
  ) {
    put_text(&mut card, "release_date", Some(date))?;
    if let Some(year) = date.get(..4).and_then(|year| year.parse::<u32>().ok()) {
      card.insert("year".into(), json!(year));
    }
  }
  if let Some(genre) =
    text(metadata, &["/genre/name"]).filter(|name| !name.eq_ignore_ascii_case("Music"))
  {
    put_text(&mut card, "genre", Some(genre))?;
    card.insert("genres".into(), json!([genre]));
  }
  let mut genres = Vec::new();
  for value in metadata
    .pointer("/genre/id")
    .into_iter()
    .chain(metadata.get("genre_ids").and_then(Value::as_array).into_iter().flatten())
    .chain(metadata.pointer("/genre/path").and_then(Value::as_array).into_iter().flatten())
    .chain(
      metadata
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| genre.get("id")),
    )
  {
    if let Some(id) = value.as_u64().or_else(|| value.as_str().and_then(|text| text.parse().ok())) {
      if !genres.contains(&id) {
        genres.push(id);
      }
      if genres.len() > 64 {
        return Err(invalid("too many genre identifiers"));
      }
    }
  }
  if !genres.is_empty() {
    card.insert("genre_ids".into(), json!(genres));
  }
  let depth = raw
    .get("maximum_bit_depth")
    .or_else(|| raw.pointer("/audio_info/maximum_bit_depth"))
    .or_else(|| metadata.get("maximum_bit_depth"))
    .and_then(Value::as_f64)
    .filter(|depth| depth.is_finite() && *depth > 0.0 && *depth <= 64.0 && depth.fract() == 0.0)
    .map(|depth| depth as u64);
  let rate = raw
    .get("maximum_sampling_rate")
    .or_else(|| raw.pointer("/audio_info/maximum_sampling_rate"))
    .or_else(|| metadata.get("maximum_sampling_rate"))
    .and_then(Value::as_f64)
    .filter(|rate| rate.is_finite() && *rate > 0.0 && *rate <= 10000.0);
  if let (Some(bits), Some(rate)) = (depth, rate) {
    card.insert(
      "quality".into(),
      json!({"codec":"flac","bits":bits,"rate":(rate * 1000.0).round() as u64}),
    );
    card.insert("hires".into(), json!(bits > 16 || rate > 48.0));
  }
  if let Some(hires) = raw.get("hires").or_else(|| metadata.get("hires")).and_then(Value::as_bool) {
    card.insert("hires".into(), json!(hires));
  }
  for (field, original) in [("available", "streamable"), ("explicit", "parental_warning")] {
    if let Some(value) = raw.get(original).and_then(Value::as_bool) {
      card.insert(field.into(), json!(value));
    }
  }
  if let Some(seconds) = raw
    .get("duration")
    .and_then(Value::as_f64)
    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
  {
    card.insert("duration_ms".into(), json!((seconds * 1000.0) as u64));
  }
  if let Some(count) = raw.get("tracks_count").and_then(Value::as_u64) {
    card.insert("tracks".into(), json!(count));
  }
  if kind == "playlist" {
    let owner =
      raw.pointer("/owner/id").and_then(|id| id.as_u64().or_else(|| id.as_str()?.parse().ok()));
    if let Some(owner) = owner {
      card.insert("owner_id".into(), json!(owner));
    }
    put_text(&mut card, "artist", text(raw, &["/owner/name/display", "/owner/name"]))?;
    card.insert("editable".into(), json!(user_id != 0 && owner == Some(user_id)));
    card.insert("copyable".into(), json!(true));
    for (target, field) in [("public", "is_public"), ("collaborative", "is_collaborative")] {
      if let Some(flag) = raw.get(field).and_then(Value::as_bool) {
        card.insert(target.into(), json!(flag));
      }
    }
    if let Some(flag) = raw
      .get("is_featured")
      .and_then(Value::as_bool)
      .or_else(|| raw.get("is_official").and_then(Value::as_bool))
    {
      card.insert("official".into(), json!(flag));
    }
  }
  Ok(Value::Object(card))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn source_rows_keep_membership_dates_and_typed_identity() {
    for (source, reference, membership, expected_date) in [
      (LibrarySource::FavoriteAlbums, "album:88", "favorite", 100),
      (LibrarySource::FavoriteTracks, "track:88", "favorite", 100),
      (LibrarySource::FollowedArtists, "artist:88", "followed", 100),
      (LibrarySource::FollowedLabels, "label:88", "followed", 100),
      (LibrarySource::FollowedAwards, "award:88", "followed", 100),
      (LibrarySource::PurchasedAlbums, "album:88", "purchased", 200),
      (LibrarySource::PurchasedTracks, "track:88", "purchased", 200),
      (LibrarySource::Playlists, "playlist:88", "favorite", 300),
    ] {
      let raw = json!({"id":88,"title":"Title","name":{"display":"Name"},"favorited_at":100,
        "purchased_at":200,"subscribed_at":"1970-01-01T00:05:00Z","created_at":400});
      let row = source.project_row(&raw, 7).unwrap();
      assert_eq!(row.card["ref"], reference);
      assert_eq!(source.membership(), membership);
      assert_eq!(row.card[membership], true);
      assert!(row.explicit);
      assert_eq!(row.added_at, expected_date);
    }
  }

  #[test]
  fn thin_cards_do_not_copy_large_detail_payloads() {
    let raw = json!({"id":9,"title":"Album","artist":{"id":7,"name":"Artist"},"favorited_at":100,
      "description":"x".repeat(50000),"performers":"x".repeat(50000),"credits":["x".repeat(50000)],
      "tracks":{"items":[{"id":1,"description":"x".repeat(50000)}]},"goodies":[{"url":"https://example.test/booklet","description":"x".repeat(50000)}],
      "image":{"large":"https://example.test/art.jpg"},"genre":{"id":42,"name":"Jazz"},"maximum_bit_depth":24,"maximum_sampling_rate":96.0});
    let row = LibrarySource::FavoriteAlbums.project_row(&raw, 7).unwrap();
    assert!(serde_json::to_vec(&row.card).unwrap().len() < 1024);
    for field in ["description", "performers", "credits", "booklets", "goodies"] {
      assert!(row.card.get(field).is_none());
    }
    assert_eq!(row.card["artist_ref"], "artist:7");
    assert_eq!(row.card["genre_ids"], json!([42]));
    assert_eq!(row.card["quality"]["rate"], 96000);
    assert_eq!(row.card["hires"], true);
  }

  #[test]
  fn playlist_ownership_uses_the_signed_in_account() {
    let raw = json!({"id":9,"name":"List","owner":{"id":7,"name":"Owner"},"is_public":true,"subscribed_at":123});
    let mine = LibrarySource::Playlists.project_row(&raw, 7).unwrap();
    assert_eq!(mine.card["editable"], true);
    assert_eq!(mine.card["owner_id"], 7);
    assert_eq!(LibrarySource::Playlists.project_row(&raw, 8).unwrap().card["editable"], false);
  }

  #[test]
  fn playlist_official_flag_uses_the_service_featured_field() {
    let raw = json!({"id":9,"name":"List","owner":{"id":0},"is_featured":true});
    let official = LibrarySource::Playlists.project_row(&raw, 7).unwrap();
    assert_eq!(official.card["official"], true);
    assert_eq!(official.card["editable"], false);
    assert_eq!(official.card["owner_id"], 0);
    let raw = json!({"id":9,"name":"List","is_featured":false,"is_official":true});
    assert_eq!(LibrarySource::Playlists.project_row(&raw, 7).unwrap().card["official"], false);
  }

  #[test]
  fn invalid_items_are_errors_and_unknown_dates_are_not_invented() {
    assert!(LibrarySource::FollowedAwards.project_row(&json!({"id":88}), 7).is_err());
    assert!(LibrarySource::FavoriteAlbums
      .project_row(&json!({"id":9,"title":"Album","favorited_at":"invalid"}), 7)
      .is_err());
    let row =
      LibrarySource::FollowedAwards.project_row(&json!({"id":88,"name":"Award"}), 7).unwrap();
    assert_eq!(row.card["kind"], "award");
    assert_eq!(row.added_at, 0);
    assert_eq!(row.card["ref"], "award:88");
  }

  #[test]
  fn source_projection_keeps_server_continuation_without_accumulating() {
    let page = LibraryPage {
      items: vec![json!({"id":9,"title":"Album","purchased_at":123})],
      next_offset: Some(1),
      total: Some(50000),
    };
    let projected = LibrarySource::PurchasedAlbums.project_page(page, 7).unwrap();
    assert_eq!(projected.items.len(), 1);
    assert_eq!(projected.next_offset, Some(1));
    assert_eq!(projected.total, Some(50000));
    assert_eq!(projected.items[0].added_at, 123);
  }

  #[test]
  fn collection_controls_use_string_choices_and_hide_conflicting_purchase_switches() {
    let genres: Vec<_> =
      (1..300).map(|id| json!({"value":id.to_string(),"label":format!("Genre {id}")})).collect();
    let controls = filter_controls("all", true, true, &genres);
    assert!(!controls.iter().any(|control| control["key"] == "hide_purchases"));
    let genre = controls.iter().find(|control| control["key"] == "genre").unwrap();
    assert_eq!(genre["options"].as_array().unwrap().len(), 256);
    let membership = controls.iter().find(|control| control["key"] == "membership").unwrap();
    let purchases = membership["options"]
      .as_array()
      .unwrap()
      .iter()
      .find(|option| option["value"] == "purchases")
      .unwrap();
    assert_eq!(purchases["excludes"], json!(["hide_purchases"]));
    for control in &controls {
      for option in control["options"].as_array().into_iter().flatten() {
        assert!(option["value"].is_string());
      }
    }
    let playlists = filter_controls("playlists", false, false, &[]);
    let owner = playlists.iter().find(|control| control["key"] == "owner").unwrap();
    assert_eq!(
      owner["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|option| option["value"].as_str().unwrap())
        .collect::<Vec<_>>(),
      vec!["all", "me", "qobuz", "other"]
    );
    assert!(!playlists.iter().any(|control| control["key"] == "genre"));
  }

  #[test]
  fn parent_projections_keep_related_identity_and_artwork_without_copying_credits() {
    let raw = json!({"id":9,"title":"Record","image":"https://images.test/record",
      "artist":{"id":1,"name":"Artist","image":"https://images.test/artist"},
      "artists":[{"id":1,"name":"Artist","roles":["main-artist"],"image":"https://images.test/artist"},
        {"id":2,"name":"Guest","roles":["performer"]},{"id":3,"name":"Engineer","roles":["engineer"]}],
      "label":{"id":4,"name":"Label","image":"https://images.test/label"},
      "genre":{"id":7,"name":"Subgenre","path":[1,7]}});
    let card = LibrarySource::FavoriteAlbums.project_row(&raw, 7).unwrap().card;
    assert_eq!(card["artist_image"], "https://images.test/artist");
    assert_eq!(card["label_image"], "https://images.test/label");
    assert_eq!(card["library_artists"].as_array().unwrap().len(), 2);
    assert_eq!(card["library_artists"][1]["ref"], "artist:2");
    assert!(card["library_artists"][1]["image"].is_null());
    assert_eq!(card["genre_ids"], json!([7, 1]));
    let artist =
      LibrarySource::FollowedArtists.project_row(&json!({"id":1,"name":"Artist"}), 7).unwrap();
    assert_eq!(artist.card["favorite"], true);
  }
}
