use crate::{
  catalog::navigation::serialized,
  catalog::{parse_reference, CatalogService, RpcFault, RpcResult},
  projection as items,
};
use serde_json::{json, Value};

fn numeric_ref(request: &Value, kind: &str) -> Result<u64, RpcFault> {
  parse_reference(request, kind)?.parse().map_err(|_| RpcFault::invalid(kind))
}
fn title(request: &Value) -> Result<&str, RpcFault> {
  request["title"]
    .as_str()
    .map(str::trim)
    .filter(|s| !s.is_empty() && s.len() <= 1000)
    .ok_or_else(|| RpcFault::invalid("playlist title"))
}
fn members(request: &Value, playlist: &qobuz_models::Playlist) -> Result<Vec<u64>, RpcFault> {
  let wanted = request["members"]
    .as_array()
    .filter(|v| !v.is_empty())
    .ok_or_else(|| RpcFault::invalid("playlist members"))?;
  let known: std::collections::HashSet<_> = playlist
    .tracks
    .as_ref()
    .into_iter()
    .flat_map(|t| &t.items)
    .filter_map(|t| t.playlist_track_id)
    .collect();
  let mut ids = vec![];
  for item in wanted {
    let id = item
      .as_str()
      .and_then(|s| s.parse().ok())
      .filter(|id| known.contains(id))
      .ok_or_else(|| RpcFault::invalid("playlist membership"))?;
    if !ids.contains(&id) {
      ids.push(id);
    }
  }
  Ok(ids)
}

pub(crate) fn card(value: &Value, user: u64) -> Value {
  let mut item = items::playlist_card(value);
  item["editable"] = json!(user != 0 && value["owner"]["id"].as_u64() == Some(user));
  item["copyable"] = json!(true);
  item["public"] = value["is_public"].clone();
  item
}
pub(crate) fn entry(value: &Value, playlist: u64, position: usize) -> Value {
  let mut item = items::track_card(value, None);
  let member = value["playlist_track_id"].as_u64();
  item["member"] = member.map(|id| json!(id.to_string())).unwrap_or(Value::Null);
  item["ref"] = json!(format!(
    "{}@playlist:{playlist}@entry:{}",
    item["ref"].as_str().unwrap_or_default(),
    member.map(|id| id.to_string()).unwrap_or_else(|| format!("position-{position}"))
  ));
  item
}
impl CatalogService {
  pub(crate) async fn edit_playlist(&self, request: Value) -> RpcResult {
    let _guard = self.write_guard.lock().await;
    let user = self
      .session
      .api
      .session()
      .await
      .ok_or_else(|| RpcFault::new(-32001, "Sign in first"))?
      .user_id;
    let action = request["action"].as_str().unwrap_or_default();
    if action == "playlist.create" {
      let playlist = self
        .session
        .api
        .create_playlist(
          title(&request)?,
          request["description"].as_str(),
          request["public"].as_bool().unwrap_or(false),
        )
        .await?;
      return Ok(
        json!({"changed_refs":[format!("playlist:{}",playlist.id)],"item":card(&serialized(playlist),user)}),
      );
    }
    let id = numeric_ref(&request, "playlist")?;
    let playlist = self.session.api.get_playlist(id).await?;
    if action == "playlist.copy" {
      let copy = self
        .session
        .api
        .create_playlist(
          title(&request)?,
          request["description"].as_str().or(playlist.description.as_deref()),
          request["public"].as_bool().unwrap_or(false),
        )
        .await?;
      let ids: Vec<_> =
        playlist.tracks.as_ref().into_iter().flat_map(|t| &t.items).map(|t| t.id).collect();
      for chunk in ids.chunks(500) {
        if self.session.api.add_tracks_to_playlist(copy.id, chunk).await.is_err() {
          return Err(RpcFault::new(-32000, format!("Playlist {} was created, but copying its tracks did not finish. Refresh your playlists before retrying.", copy.id)));
        }
      }
      return Ok(
        json!({"changed_refs":[format!("playlist:{}",copy.id)],"item":card(&serialized(copy),user)}),
      );
    }
    if playlist.owner.id != user || user == 0 {
      return Err(RpcFault::new(-32003, "Only the playlist owner can edit it"));
    }
    match action {
      "playlist.update" => {
        self
          .session
          .api
          .update_playlist(
            id,
            Some(title(&request)?),
            request["description"].as_str(),
            request["public"].as_bool(),
          )
          .await?;
      }
      "playlist.delete" => {
        self.session.api.delete_playlist(id).await?;
      }
      "playlist.add" => {
        let tracks = request["tracks"]
          .as_array()
          .filter(|v| !v.is_empty() && v.len() <= 500)
          .ok_or_else(|| RpcFault::invalid("playlist tracks"))?;
        let ids: Vec<_> = tracks
          .iter()
          .map(|r| numeric_ref(&json!({"ref":r}), "track"))
          .collect::<Result<_, _>>()?;
        self.session.api.add_tracks_to_playlist(id, &ids).await?;
      }
      "playlist.remove" => {
        self.session.api.remove_tracks_from_playlist(id, &members(&request, &playlist)?).await?;
      }
      "playlist.move" => {
        let count = playlist.tracks.as_ref().map_or(0, |t| t.items.len());
        let before = request["before"]
          .as_u64()
          .filter(|n| *n <= count as u64)
          .and_then(|n| u32::try_from(n).ok())
          .ok_or_else(|| RpcFault::invalid("playlist position"))?;
        self
          .session
          .api
          .update_playlist_tracks_position(id, &members(&request, &playlist)?, before)
          .await?;
      }
      _ => return Err(RpcFault::invalid("playlist action")),
    }
    Ok(json!({"changed_refs":[format!("playlist:{id}")]}))
  }
}
#[cfg(test)]
#[path = "checks/edits_tests.rs"]
mod checks;
