use crate::{
  catalog::navigation::serialized,
  catalog::{CatalogService, RpcFault, RpcResult},
  projection as items,
};
use qobuz_models::{Track, TrackToAnalyse};
use serde_json::{json, Value};
use std::time::Duration;
impl CatalogService {
  pub(crate) async fn load_mix(&self, reference: &str) -> RpcResult {
    self.require_session().await?;
    self.memoized(&format!("mix {reference}"), Duration::from_secs(300), || async {
            let (kind, seed) = reference.split_once(':').ok_or_else(|| RpcFault::invalid("mix"))?;
            let (title, tracks) = if kind == "radio" {
                let (entity, id) = seed.split_once(':').ok_or_else(|| RpcFault::invalid("radio seed"))?;
                let radio = match entity { "album" => self.session.api.get_radio_album(id).await?, "artist" => self.session.api.get_radio_artist(id).await?, "track" => self.session.api.get_radio_track(id).await?, _ => return Err(RpcFault::invalid("radio seed")) };
                let bare = radio.tracks.items;
                let ids: Vec<u64> = bare.iter().map(|t| t.id).collect();
                let full = match self.session.api.get_tracks_batch(&ids).await { Ok(full) if !full.is_empty() => full, _ => bare };
                (radio.title.unwrap_or_else(|| "Radio".into()), full)
            } else {
                let title = match seed { "daily" => "DailyQ", "weekly" => "WeeklyQ", "fav" => "FavQ", "top" => "TopQ", _ => return Err(RpcFault::invalid("mix")) };
                (title.into(), self.generate_mix(seed).await?)
            };
            let rows: Vec<Value> = tracks.iter().map(|t| items::track_card(&serialized(t), None)).collect();
            Ok(json!({"playlist":{"kind":"playlist","ref":reference,"title":title,"tracks":rows.len(),"refreshable":kind=="mix"},"tracks":rows,"total":rows.len()}))
        }).await
  }
  async fn generate_mix(&self, kind: &str) -> Result<Vec<Track>, RpcFault> {
    if kind == "top" {
      let lists = self.session.api.get_user_playlists().await?;
      let mut tracks = vec![];
      let mut jobs = tokio::task::JoinSet::new();
      for (position, list) in lists.into_iter().take(5).enumerate() {
        let client = self.session.api.clone();
        jobs.spawn(async move { (position, client.get_playlist(list.id).await) });
      }
      let mut pages = Vec::new();
      while let Some(result) = jobs.join_next().await {
        if let Ok((position, Ok(playlist))) = result {
          pages.push((position, playlist));
        }
      }
      pages.sort_by_key(|(position, _)| *position);
      for (_, playlist) in pages {
        if let Some(page) = playlist.tracks {
          tracks.extend(page.items);
        }
        if tracks.len() >= 100 {
          break;
        }
      }
      tracks.truncate(100);
      return Ok(tracks);
    }
    let favorites = self.session.api.get_favorites("tracks", 200, 0).await?;
    let mut tracks: Vec<Track> = items::collection(favorites.get("tracks"))
      .into_iter()
      .filter_map(|v| serde_json::from_value(v).ok())
      .collect();
    if kind == "fav" {
      let mut seed = u128::from_be_bytes(*uuid::Uuid::new_v4().as_bytes()) as u64;
      for n in (1..tracks.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        tracks.swap(n, seed as usize % (n + 1));
      }
      return Ok(tracks);
    }
    let mut seeds: Vec<_> = self.recent_ids().await;
    for track in &tracks {
      if !seeds.contains(&track.id) {
        seeds.push(track.id);
      }
    }
    seeds.truncate(200);
    if seeds.is_empty() {
      return Ok(vec![]);
    }
    let selected: Vec<_> =
      seeds.iter().step_by((seeds.len() / 9).max(1)).take(9).copied().collect();
    let analysis_tracks = self.session.api.get_tracks_batch(&selected).await?;
    let analysed: Vec<_> = analysis_tracks
      .iter()
      .filter_map(|track| {
        let artist = track.performer.as_ref().or(track.composer.as_ref())?;
        Some(TrackToAnalyse {
          track_id: track.id,
          artist_id: artist.id,
          genre_id: track.album.as_ref().and_then(|a| a.genre.as_ref()).map_or(0, |g| g.id),
          label_id: track.album.as_ref().and_then(|a| a.label.as_ref()).map_or(0, |l| l.id),
        })
      })
      .collect();
    let result = self
      .session
      .api
      .get_dynamic_suggest_full(&seeds, &analysed, (50 - analysed.len()) as u32)
      .await?;
    if result.is_empty() {
      Ok(self.session.api.get_dynamic_suggest(&seeds, 50).await?)
    } else {
      Ok(result)
    }
  }
}
