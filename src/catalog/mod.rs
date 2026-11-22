pub(crate) use crate::runtime::{RpcFault, RpcResult};
use crate::{memo::ReplyMemo, projection as items, runtime::SessionContext};
use qobuz_models::Quality;
use serde_json::{json, Value};
use std::{
  sync::Arc,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

const PAGE_FRESH: Duration = Duration::from_secs(300);
const LIBRARY_FRESH: Duration = Duration::from_secs(60);
const CATALOG_FRESH: Duration = Duration::from_secs(600);
const PAGE_MOST: u64 = 500;

pub(crate) struct CatalogService {
  pub(crate) session: Arc<SessionContext>,
  pub(crate) write_guard: tokio::sync::Mutex<()>,
  pub(crate) memo: ReplyMemo,
  playback_metadata: playback_metadata::PlaybackMetadata,
  pub(crate) library: crate::library_catalog::LibraryCatalog,
  pub(crate) favorites: favorites::FavoriteLedger,
  pub(crate) magazine: crate::magazine::Magazine,
}

impl CatalogService {
  pub(crate) fn new(session: Arc<SessionContext>) -> Self {
    Self {
      session,
      write_guard: tokio::sync::Mutex::new(()),
      memo: ReplyMemo::default(),
      playback_metadata: Default::default(),
      library: Default::default(),
      favorites: Default::default(),
      magazine: Default::default(),
    }
  }
  pub(crate) fn invalidate_catalog(&self) {
    self.memo.clear();
    self.playback_metadata.clear();
    self.library.invalidate();
    self.magazine.reset();
  }
  pub(crate) fn invalidate_mix(&self, reference: &str) {
    self.memo.remove_prefix(&format!("mix {reference}"));
  }
  pub(crate) async fn require_session(&self) -> Result<(), RpcFault> {
    self.session.require_identity().await
  }
  async fn account(&self) -> u64 {
    self.session.api.session().await.map_or(0, |s| s.user_id)
  }
  /// States the heart on a reply's own card, outside the memo so it stays current.
  async fn with_heart(&self, mut reply: Value, card: &str) -> RpcResult {
    self.favorites.mark(self.account().await, reply.get_mut(card));
    Ok(reply)
  }
  pub(crate) async fn memoized<F, Fut>(&self, key: &str, fresh: Duration, read: F) -> RpcResult
  where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = RpcResult>,
  {
    if let Some(value) = self.memo.get(key) {
      return Ok(value);
    }
    let _reservation = self.memo.reserve(key).await;
    if let Some(value) = self.memo.get(key) {
      return Ok(value);
    }
    let value = read().await?;
    self.playback_metadata.remember(&value, fresh);
    self.memo.put(key, fresh, &value);
    Ok(value)
  }
  pub(crate) async fn load_home(&self) -> RpcResult {
    self.require_session().await?;
    let discover =
      serde_json::to_value(self.session.api.get_discover_index(None).await?).unwrap_or_default();
    let shelves = [
      ("new_releases", "New releases"),
      ("qobuzissims", "Qobuzissimes"),
      ("album_of_the_week", "Album of the week"),
      ("most_streamed", "Most streamed"),
      ("press_awards", "Press awards"),
      ("ideal_discography", "Ideal discography"),
      ("playlists", "Playlists"),
    ];
    let shelves: Vec<Value> = shelves.iter().filter_map(|(key, title)| {
            let found = discover.pointer(&format!("/containers/{key}/data/items"))?.as_array()?;
            let playlists = *key == "playlists";
            let items: Vec<Value> = found.iter().map(|v| if playlists { items::playlist_card(v) } else { items::album_card(v) }).collect();
            (!items.is_empty()).then(|| json!({"id": key, "title": title, "kind": if playlists { "playlist" } else { "album" }, "items": items}))
        }).collect();
    Ok(json!({"shelves": shelves}))
  }

  pub(crate) async fn list_library(&self, request: Value) -> RpcResult {
    let kind = request.get("kind").and_then(Value::as_str).unwrap_or("albums").to_string();
    let offset = request.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(PAGE_MOST).min(PAGE_MOST);
    let key = format!("library {kind} {offset} {limit}");
    self
      .memoized(&key, LIBRARY_FRESH, || async {
        self.require_session().await?;
        if kind == "playlists" {
          let user = self.account().await;
          let all: Vec<Value> = self
            .session
            .api
            .get_user_playlists()
            .await?
            .iter()
            .map(|p| {
              let mut card =
                crate::catalog::edits::card(&serde_json::to_value(p).unwrap_or_default(), user);
              // A playlist the account follows is its favourite; its own is not.
              card["favorite"] = json!(card["editable"] != true);
              card
            })
            .collect();
          let followed = all
            .iter()
            .filter(|card| card["favorite"] == true)
            .filter_map(|card| card["ref"].as_str());
          self.favorites.listed(user, "playlist", 0, followed, true);
          let total = all.len();
          let page: Vec<Value> =
            all.into_iter().skip(offset as usize).take(limit as usize).collect();
          return Ok(json!({"items": page, "total": total}));
        }
        let map = match kind.as_str() {
          "albums" => items::album_card,
          "artists" => items::artist_card,
          "labels" => items::label_card,
          "tracks" => |v: &Value| items::track_card(v, None),
          _ => return Err(RpcFault::invalid("library kind")),
        };
        let favorites = self.session.api.get_favorites(&kind, limit as u32, offset as u32).await?;
        let list = favorites.get(&kind);
        let total = list.and_then(|l| l.get("total")).and_then(Value::as_u64).unwrap_or(0);
        // This list is the account's favourites, so every item says so.
        let page: Vec<Value> = items::collection(list)
          .iter()
          .map(|raw| {
            let mut card = map(raw);
            card["favorite"] = json!(true);
            card
          })
          .collect();
        let complete = offset + page.len() as u64 >= total;
        self.favorites.listed(
          self.account().await,
          kind.trim_end_matches('s'),
          offset,
          page.iter().filter_map(|card| card["ref"].as_str()),
          complete,
        );
        Ok(json!({"items": page, "total": total}))
      })
      .await
  }

  pub(crate) async fn load_album(&self, request: Value) -> RpcResult {
    let id = parse_reference(&request, "album")?;
    let reply = self
      .memoized(&format!("album {id}"), CATALOG_FRESH, || async {
        self.require_session().await?;
        let record = match self.session.api.get_album(&id).await {
          Ok(record) => crate::catalog::navigation::serialized(record),
          Err(error) if error.is_album_unavailable(&id) => {
            return Ok(json!({"availability":"unavailable","tracks":[]}))
          }
          Err(error) => return Err(error.into()),
        };
        let tracks: Vec<Value> = items::collection(record.get("tracks"))
          .iter()
          .map(|t| items::track_card(t, Some(&record)))
          .collect();
        let mut album = items::album_card(&record);
        if let Some(description) = record.get("description").filter(|d| d.is_string()) {
          album["description"] = description.clone();
        }
        album["credits"] = record["artists"].clone();
        album["track_credits"] = json!(tracks
          .iter()
          .filter(|t| t["performers"].as_str().is_some_and(|p| !p.is_empty()))
          .map(|t| json!({"title":t["title"],"performers":t["performers"]}))
          .collect::<Vec<_>>());

        Ok(json!({"album": album, "tracks": tracks}))
      })
      .await?;
    self.with_heart(reply, "album").await
  }

  pub(crate) async fn load_artist(&self, request: Value) -> RpcResult {
    let id: u64 =
      parse_reference(&request, "artist")?.parse().map_err(|_| RpcFault::invalid("artist"))?;
    let reply = self.memoized(&format!("artist {id}"), CATALOG_FRESH, || async {
            self.require_session().await?;
            let found = serde_json::to_value(self.session.api.get_artist(id, true).await?).unwrap_or_default();
            let albums: Vec<Value> = items::collection(found.get("albums")).iter().map(items::album_card).collect();
            let mut artist = items::artist_card(&found);
            if let Some(biography) = found.pointer("/biography/content").filter(|b| b.is_string()) {
                artist["biography"] = biography.clone();
            }
            Ok(json!({"artist": artist, "shelves": [{"id": "albums", "title": "Albums", "kind": "album", "items": albums}]}))
        }).await?;
    self.with_heart(reply, "artist").await
  }

  pub(crate) async fn load_playlist(&self, request: Value) -> RpcResult {
    if let Some(reference) =
      request["ref"].as_str().filter(|r| r.starts_with("mix:") || r.starts_with("radio:"))
    {
      return self.load_mix(reference).await;
    }
    let id: u64 =
      parse_reference(&request, "playlist")?.parse().map_err(|_| RpcFault::invalid("playlist"))?;
    let reply = self.memoized(&format!("playlist {id}"), PAGE_FRESH, || async {
            self.require_session().await?;
            let found =
                serde_json::to_value(self.session.api.get_playlist(id).await?).unwrap_or_default();
            let tracks: Vec<Value> = items::collection(found.get("tracks"))
                .iter().enumerate()
                .map(|(at, t)| crate::catalog::edits::entry(t, id, at))
                .collect();
            let total = tracks.len();
            let user = self.session.api.session().await.map_or(0, |s| s.user_id);
            Ok(json!({"playlist": crate::catalog::edits::card(&found, user), "tracks": tracks, "total": total}))
        })
        .await?;
    self.with_heart(reply, "playlist").await
  }

  pub(crate) async fn search_catalog(&self, request: Value) -> RpcResult {
    let text = request.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if text.is_empty() {
      return Ok(json!({"albums": [], "tracks": [], "artists": [], "playlists": []}));
    }
    let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(20).min(100);
    self
      .memoized(&format!("search {} {limit}", text.to_lowercase()), CATALOG_FRESH, || async {
        self.require_session().await?;
        let found = self.session.api.catalog_search(&text, limit as u32, 0).await?;
        let read = |key: &str, map: fn(&Value) -> Value| -> Vec<Value> {
          items::collection(found.get(key)).iter().map(map).collect()
        };
        Ok(json!({
            "albums": read("albums", items::album_card),
            "tracks": read("tracks", |v| items::track_card(v, None)),
            "artists": read("artists", items::artist_card),
            "playlists": read("playlists", items::playlist_card),
        }))
      })
      .await
  }

  pub(crate) async fn resolve_track(&self, request: Value) -> RpcResult {
    let id: u64 =
      parse_reference(&request, "track")?.parse().map_err(|_| RpcFault::invalid("track"))?;
    let rate = request.pointer("/output/max_rate").and_then(Value::as_u64).unwrap_or(192_000);
    let bits = request.pointer("/output/max_bits").and_then(Value::as_u64).unwrap_or(24);
    let quality = if bits < 24 {
      Quality::Lossless
    } else if rate > 96_000 {
      Quality::UltraHiRes
    } else {
      Quality::HiRes
    };
    self.require_session().await?;
    let metadata = self.playback_metadata.get(id);
    let normalization = metadata.as_ref().and_then(|value| value.get("normalization")).cloned();
    let (resolved, track) =
      tokio::join!(self.session.access.resolve(id, quality.id() as u32, metadata), async {
        match normalization {
          Some(value) => Ok(json!({"normalization":value})),
          None => self.lookup_track(id).await,
        }
      });
    let mut answer = resolved.map_err(|message| RpcFault::new(-32000, message))?;
    let format = &answer["format"];
    let actual_rate = format["sample_rate"].as_u64().unwrap_or(0);
    let actual_bits = format["bits"].as_u64().unwrap_or(0);
    let audio = qobuz_models::StreamUrl {
      blob: None,
      duration: format["frames"]
        .as_f64()
        .zip(format["sample_rate"].as_f64())
        .map(|(frames, rate)| frames / rate),
      url: answer["url"].as_str().unwrap_or_default().to_string(),
      format_id: if actual_rate == 0 {
        0
      } else if actual_bits <= 16 {
        6
      } else if actual_rate <= 96_000 {
        7
      } else {
        27
      },
      mime_type: "audio/wav".into(),
      sampling_rate: actual_rate as f64 / 1000.0,
      bit_depth: Some(actual_bits as u32),
      track_id: id,
      restrictions: vec![],
      sample: answer["preview"] == true,
    };
    let context = request["ref"].as_str().and_then(|r| r.split('@').nth(1)).map(str::to_string);
    answer["resolution_id"] =
      json!(self.session.journal.issue_ticket(self.account().await, &audio, context).await);
    answer["normalization"] =
      track.ok().and_then(|track| track.get("normalization").cloned()).unwrap_or(Value::Null);
    Ok(answer)
  }

  pub(crate) async fn lookup_track(&self, id: u64) -> RpcResult {
    self
      .memoized(&format!("track {id}"), CATALOG_FRESH, || async {
        let track = self.session.api.get_track(id).await?;
        Ok(items::track_card(&crate::catalog::navigation::serialized(track), None))
      })
      .await
  }

  /// A favourite write whose answer waits for the library to hold it.
  pub(crate) async fn set_favorite(&self, request: Value) -> RpcResult {
    let reply = self.write_favorite(request).await?;
    self.library.settle(&self.session).await;
    Ok(reply)
  }

  pub(crate) async fn write_favorite(&self, request: Value) -> RpcResult {
    let reference =
      request.get("ref").and_then(Value::as_str).ok_or_else(|| RpcFault::invalid("ref"))?;
    let (kind, id) = reference.split_once(':').ok_or_else(|| RpcFault::invalid("ref"))?;
    let id = id.split('@').next().unwrap_or(id);
    if !matches!(kind, "album" | "track" | "artist" | "label" | "award" | "playlist") {
      return Err(RpcFault::invalid("favorite kind"));
    }
    self.require_session().await?;
    let enabled = request.get("on").and_then(Value::as_bool).unwrap_or(true);
    let playlist = if kind == "playlist" {
      Some(
        id.parse::<u64>()
          .ok()
          .filter(|id| *id != 0)
          .ok_or_else(|| RpcFault::invalid("playlist"))?,
      )
    } else {
      None
    };
    let _mutation = if playlist.is_some() {
      self.library.playlists_changed(&self.session).await?
    } else {
      self.library.favorite_changed(&self.session, kind).await?
    };
    let outcome = if let Some(identifier) = playlist {
      if enabled {
        self.session.api.subscribe_playlist(identifier).await
      } else {
        self.session.api.unsubscribe_playlist(identifier).await
      }
    } else if enabled {
      self.session.api.add_favorite(kind, id).await
    } else {
      self.session.api.remove_favorite(kind, id).await
    };
    outcome?;
    self.favorites.changed(self.account().await, &format!("{kind}:{id}"), enabled);
    self.memo.clear();
    Ok(json!({}))
  }
}

pub(crate) fn classify_link(url: &str) -> Value {
  use qobuz_api::ResolvedLink;
  match qobuz_api::resolve_link(url) {
    Ok(ResolvedLink::OpenAlbum(id)) => json!({"kind": "album", "ref": format!("album:{id}")}),
    Ok(ResolvedLink::OpenTrack(id)) => json!({"kind": "track", "ref": format!("track:{id}")}),
    Ok(ResolvedLink::OpenArtist(id)) => json!({"kind": "artist", "ref": format!("artist:{id}")}),
    Ok(ResolvedLink::OpenPlaylist(id)) => {
      json!({"kind": "playlist", "ref": format!("playlist:{id}")})
    }
    Err(_) => Value::Null,
  }
}

pub(crate) fn parse_reference(request: &Value, kind: &str) -> Result<String, RpcFault> {
  request
    .get("ref")
    .and_then(Value::as_str)
    .and_then(|r| r.split('@').next()?.strip_prefix(kind)?.strip_prefix(':'))
    .filter(|id| !id.is_empty())
    .map(str::to_string)
    .ok_or_else(|| RpcFault::invalid(kind))
}

pub(crate) fn now() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
#[path = "checks/mod_tests.rs"]
mod checks;

mod browsing;
mod collections;
mod curation;
mod edits;
mod favorites;
pub(crate) mod navigation;
mod playback_metadata;
mod subtitles;
