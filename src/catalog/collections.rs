use crate::{
  catalog::navigation::{serialized, shelf},
  catalog::CatalogService,
  projection as items,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
impl CatalogService {
  pub(crate) async fn load_pins(&self) -> Vec<Value> {
    let Some(session) = self.session.api.session().await else {
      return vec![];
    };
    crate::files::load_json(&self.session.directory.join(format!("pins-{}.json", session.user_id)))
      .ok()
      .flatten()
      .unwrap_or_default()
  }
  pub(crate) async fn dispatch_action(&self, request: Value) -> crate::catalog::RpcResult {
    self.require_session().await?;
    if request["action"] == "playlist.search" {
      let found =
        self.search_catalog(serde_json::json!({"text":request["text"],"limit":30})).await?;
      return Ok(serde_json::json!({"read_only":true,"items":found["tracks"]}));
    }
    if request["action"] == "mix.refresh" {
      let reference = request["ref"]
        .as_str()
        .filter(|value| matches!(*value, "mix:daily" | "mix:weekly" | "mix:fav" | "mix:top"))
        .ok_or_else(|| crate::catalog::RpcFault::invalid("mix"))?;
      self.invalidate_mix(reference);
      return Ok(serde_json::json!({"changed_refs":[reference]}));
    }
    let action = request["action"].as_str().unwrap_or_default();
    let _mutation = if matches!(
      action,
      "playlist.create"
        | "playlist.copy"
        | "playlist.update"
        | "playlist.delete"
        | "playlist.add"
        | "playlist.remove"
        | "playlist.move"
    ) {
      Some(self.library.playlists_changed(&self.session).await?)
    } else {
      None
    };
    let result = match action {
      action if action.starts_with("playlist.") => self.edit_playlist(request.clone()).await,
      "favorite" => self.write_favorite(request.clone()).await,
      "pin" => {
        let _guard = self.write_guard.lock().await;
        let user = self
          .session
          .api
          .session()
          .await
          .ok_or_else(|| crate::catalog::RpcFault::new(-32001, "Sign in first"))?
          .user_id;
        let item = &request["item"];
        let reference = item["ref"]
          .as_str()
          .filter(|r| r.contains(':'))
          .ok_or_else(|| crate::catalog::RpcFault::invalid("pin"))?;
        let mut pins = self.load_pins().await;
        pins.retain(|p| p["ref"] != reference);
        if request["on"] == true {
          pins.push(item.clone());
        }
        crate::files::store_json(&self.session.directory.join(format!("pins-{user}.json")), &pins)
          .map_err(|e| crate::catalog::RpcFault::new(-32000, e))?;
        Ok(serde_json::json!({}))
      }
      _ => Err(crate::catalog::RpcFault::invalid("page action")),
    };
    self.invalidate_catalog();
    if action == "favorite" && result.is_ok() {
      self.library.settle(&self.session).await;
    }
    result
  }

  pub(crate) async fn recent_ids(&self) -> Vec<u64> {
    let Some(session) = self.session.api.session().await else {
      return vec![];
    };
    let mut seen = HashSet::new();
    self
      .session
      .journal
      .recent(session.user_id)
      .await
      .iter()
      .filter_map(|v| v["track_id"].as_u64())
      .filter(|id| seen.insert(*id))
      .take(200)
      .collect()
  }
  pub(crate) async fn history_rails(&self) -> Vec<Value> {
    let Some(session) = self.session.api.session().await else {
      return vec![];
    };
    let history = self.session.journal.recent(session.user_id).await;
    let ids = self.recent_ids().await;
    let tracks = if ids.is_empty() {
      vec![]
    } else {
      self
        .session
        .api
        .get_tracks_batch(&ids.iter().copied().take(100).collect::<Vec<_>>())
        .await
        .unwrap_or_default()
    };
    let by_id: HashMap<_, _> = tracks.iter().map(|t| (t.id, t)).collect();
    let mut albums = vec![];
    let mut seen = HashSet::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for occurrence in &history {
      if let Some(track) = occurrence["track_id"].as_u64().and_then(|id| by_id.get(&id)) {
        if let Some(album) = &track.album {
          *counts.entry(album.id.clone()).or_default() += 1;
          if !occurrence["context"].as_str().is_some_and(|c| c.starts_with("playlist:"))
            && seen.insert(album.id.clone())
          {
            albums.push(items::album_card(&serialized(album)));
          }
        }
      }
    }
    let mut most = albums.clone();
    most.sort_by_key(|a| {
      std::cmp::Reverse(
        counts
          .get(a["ref"].as_str().unwrap_or_default().trim_start_matches("album:"))
          .copied()
          .unwrap_or_default(),
      )
    });
    let continued: Vec<_> = ids
      .iter()
      .take(20)
      .filter_map(|id| by_id.get(id))
      .map(|t| items::track_card(&serialized(t), None))
      .collect();
    let mut playlist_ids = HashSet::new();
    let mut playlist_jobs = tokio::task::JoinSet::new();
    for row in &history {
      if let Some(id) = row["context"]
        .as_str()
        .and_then(|s| s.strip_prefix("playlist:"))
        .and_then(|s| s.parse::<u64>().ok())
      {
        if playlist_ids.insert(id) {
          let client = self.session.api.clone();
          let position = playlist_jobs.len();
          playlist_jobs.spawn(async move { (position, client.get_playlist(id).await) });
        }
        if playlist_jobs.len() >= 12 {
          break;
        }
      }
    }
    let mut playlist_rows = vec![];
    while let Some(result) = playlist_jobs.join_next().await {
      if let Ok((position, Ok(playlist))) = result {
        playlist_rows.push((position, items::playlist_card(&serialized(playlist))));
      }
    }
    playlist_rows.sort_by_key(|(position, _)| *position);
    let playlists = playlist_rows.into_iter().map(|(_, playlist)| playlist).collect();
    vec![
      shelf("recent_albums", "Recently played albums", "album", albums, None),
      shelf("continue", "Continue listening", "track", continued, None),
      shelf("most_played", "Most played albums", "album", most, None),
      shelf("recent_playlists", "Recently played playlists", "playlist", playlists, None),
    ]
  }
}
