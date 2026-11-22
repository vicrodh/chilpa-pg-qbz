use crate::{
  catalog::navigation::{mapped, serialized, shelf},
  catalog::{CatalogService, RpcFault, RpcResult},
  projection as items,
};
use serde_json::{json, Value};

const EDITORIAL: &[(&str, &str, &str)] = &[
  ("new_releases", "New releases", "newReleases"),
  ("album_of_the_week", "Albums of the week", "albumOfTheWeek"),
  ("qobuzissims", "Qobuzissimes", "qobuzissims"),
  ("press_awards", "Press accolades", "pressAward"),
  ("most_streamed", "Most streamed", "mostStreamed"),
  ("ideal_discography", "Ideal discography", "idealDiscography"),
  ("playlists", "Qobuz playlists", "playlists"),
];
fn genres(raw: &str) -> Option<Vec<u64>> {
  let ids: Vec<_> = raw.split(',').filter_map(|v| v.parse().ok()).collect();
  (!ids.is_empty()).then_some(ids)
}
fn spotlight_artist(artists: &[Value]) -> Option<u64> {
  artists.iter().find_map(|artist| artist["ref"].as_str()?.strip_prefix("artist:")?.parse().ok())
}
impl CatalogService {
  pub(crate) async fn genre_discovery(&self, request: &Value) -> RpcResult {
    let name = request["filters"]["genre"]
      .as_str()
      .map(str::trim)
      .filter(|name| !name.is_empty() && name.len() <= 1024)
      .ok_or_else(|| RpcFault::invalid("genre name"))?;
    let limit = request["limit"].as_u64().unwrap_or(60).clamp(1, 60) as u32;
    self.require_session().await?;
    let genres = self
      .memoized("genres", std::time::Duration::from_secs(86400), || async {
        Ok(serialized(self.session.api.get_genres(None).await?))
      })
      .await?;
    let folded = name.to_lowercase();
    let id = genres
      .as_array()
      .into_iter()
      .flatten()
      .find(|genre| genre["name"].as_str().is_some_and(|name| name.trim().to_lowercase() == folded))
      .and_then(|genre| genre["id"].as_u64());
    let Some(id) = id else {
      return Ok(json!({"shelves":[]}));
    };
    let read = |endpoint| async move {
      self.session.api.get_discover_albums(endpoint, Some(vec![id]), 0, limit).await
    };
    let (new, featured) =
      tokio::join!(read("/discover/newReleases"), read("/discover/mostStreamed"));
    let (new, featured) = match (new, featured) {
      (Err(error), Err(_)) => return Err(error.into()),
      (new, featured) => (new.ok(), featured.ok()),
    };
    let mut shelves = Vec::new();
    if let Some(new) = &new {
      let rows = new.items.iter().map(|row| items::album_card(&serialized(row))).collect();
      shelves.push(shelf("new_releases", "New releases", "album", rows, None));
    }
    let mut seen = std::collections::HashSet::new();
    let artists = featured
      .iter()
      .chain(new.iter())
      .flat_map(|page| &page.items)
      .flat_map(|album| &album.artists)
      .filter(|artist| artist.id != 0 && seen.insert(artist.id))
      .take(limit as usize)
      .map(|artist| items::artist_card(&serialized(artist)))
      .collect();
    shelves.push(shelf("artists", "Artists", "artist", artists, None));
    Ok(json!({"shelves":shelves}))
  }

  pub(crate) async fn load_discovery(&self, parts: &[&str]) -> RpcResult {
    let tab = parts.get(1).copied().unwrap_or("home");
    if !matches!(tab, "home" | "editors" | "foryou") {
      return Err(RpcFault::invalid("discover tab"));
    }
    let filter = parts.get(2).copied().unwrap_or("");
    let editorial_key = format!("editorial {filter}");
    let (index, genre_list, personal) = tokio::join!(
      self.memoized(&editorial_key, std::time::Duration::from_secs(300), || async {
        Ok(serialized(self.session.api.get_discover_index(genres(filter)).await?))
      }),
      self.memoized("genres", std::time::Duration::from_secs(86400), || async {
        Ok(serialized(self.session.api.get_genres(None).await?))
      }),
      async {
        if tab == "editors" {
          Ok(json!([]))
        } else {
          let deep = tab == "foryou";
          self
            .memoized(
              if deep { "personal-foryou" } else { "personal-home" },
              std::time::Duration::from_secs(300),
              || async { Ok(json!(self.recommendation_rails(deep).await?)) },
            )
            .await
        }
      }
    );
    let index = index?;
    let mut shelves = vec![];
    for (field, title, endpoint) in EDITORIAL {
      let container = &index["containers"][field]["data"];
      let kind = if *field == "playlists" { "playlist" } else { "album" };
      let rows = mapped(
        container,
        "items",
        if kind == "playlist" { items::playlist_card } else { items::album_card },
      );
      let mut row = shelf(field, title, kind, rows, Some(format!("browse:{endpoint}:{filter}:0")));
      if *field == "playlists" {
        row["tags"] = index["containers"]["playlists_tags"]["data"]["items"].clone();
      }
      shelves.push(row);
    }
    if tab != "editors" {
      match personal { Ok(value) => shelves.extend(value.as_array().into_iter().flatten().cloned()), Err(_) => shelves.push(json!({"id":"personal_error","title":"For You","error":"Personal recommendations could not be loaded. Retry this page.","items":[]})) };
    }
    let order: &[(&str, bool)] = match tab {
      "editors" => &[
        ("new_releases", true),
        ("album_of_the_week", true),
        ("qobuzissims", true),
        ("press_awards", true),
        ("most_streamed", true),
        ("ideal_discography", true),
        ("playlists", true),
      ],
      "foryou" => &[
        ("mixes", true),
        ("pinned", true),
        ("release_watch", true),
        ("radio", true),
        ("continue", true),
        ("recent_albums", true),
        ("recent_playlists", true),
        ("top_artists", true),
        ("favorite_albums", true),
        ("similar", true),
        ("rediscover", true),
        ("essentials", true),
        ("follow", true),
        ("spotlight", true),
        ("most_played", false),
      ],
      _ => &[
        ("new_releases", true),
        ("press_awards", true),
        ("playlists", true),
        ("recent_albums", true),
        ("recent_playlists", true),
        ("continue", true),
        ("ideal_discography", true),
        ("most_streamed", true),
        ("pinned", true),
        ("mixes", false),
        ("release_watch", false),
        ("album_of_the_week", false),
        ("qobuzissims", false),
        ("top_artists", false),
        ("favorite_albums", false),
        ("most_played", false),
      ],
    };
    let errors: Vec<_> = shelves.iter().filter(|s| s.get("error").is_some()).cloned().collect();
    shelves = order
      .iter()
      .filter_map(|(id, enabled)| {
        let mut row = shelves.iter().find(|s| s["id"] == *id)?.clone();
        row["enabled"] = json!(enabled);
        Some(row)
      })
      .collect();
    for error in errors {
      if !shelves.iter().any(|s| s["id"] == error["id"]) {
        shelves.push(error);
      }
    }
    if tab == "home" {
      for row in &mut shelves {
        if row["id"] == "most_streamed" {
          row["presentation"] = json!("ranked");
        }
      }
    }
    let tabs: Vec<_> = [("home", "Home"), ("editors", "Editor's Picks"), ("foryou", "For You")].into_iter()
            .map(|(id, title)| json!({"title":title,"ref":format!("discover:{id}:{filter}"),"active":id==tab})).collect();
    Ok(
      json!({"title":"Discover","personal":tab != "editors","tab":tab,"tabs":tabs,"genres":genre_list.unwrap_or_default(),"genre_filter":filter,"shelves":shelves}),
    )
  }
  pub(crate) async fn browse_discovery(&self, parts: &[&str]) -> RpcResult {
    let endpoint = parts.get(1).copied().unwrap_or_default();
    if !EDITORIAL.iter().any(|(_, _, e)| *e == endpoint) {
      return Err(RpcFault::invalid("discover section"));
    }
    let filter = parts.get(2).copied().unwrap_or_default();
    let offset: u32 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or_default();
    let page = if endpoint == "playlists" {
      serialized(
        self
          .session
          .api
          .get_discover_playlists(None, genres(filter), Some(100), Some(offset))
          .await?,
      )
    } else {
      serialized(
        self
          .session
          .api
          .get_discover_albums(&format!("/discover/{endpoint}"), genres(filter), offset, 100)
          .await?,
      )
    };
    let raw = if page.get("items").is_some() { &page } else { &page["data"] };
    let rows = mapped(
      raw,
      "items",
      if endpoint == "playlists" { items::playlist_card } else { items::album_card },
    );
    let next = (raw["has_more"] == true && !rows.is_empty())
      .then(|| format!("browse:{endpoint}:{filter}:{}", offset + rows.len() as u32));
    let title =
      EDITORIAL.iter().find(|(_, _, e)| *e == endpoint).map(|(_, t, _)| *t).unwrap_or_default();
    Ok(json!({"title":title,"items":rows,"next":next}))
  }
  async fn recommendation_rails(&self, deep: bool) -> Result<Vec<Value>, RpcFault> {
    let fresh = std::time::Duration::from_secs(300);
    let limit = if deep { 500 } else { 50 };
    let (albums, artists, releases, history) = tokio::join!(
      self.memoized(
        if deep { "favorite-albums-full" } else { "favorite-albums-preview" },
        fresh,
        || async { Ok(self.session.api.get_favorites("albums", limit, 0).await?) }
      ),
      self.memoized(
        if deep { "favorite-artists-full" } else { "favorite-artists-preview" },
        fresh,
        || async { Ok(self.session.api.get_favorites("artists", limit, 0).await?) }
      ),
      self.memoized("release-watch", fresh, || async {
        Ok(serialized(self.session.api.get_release_watch("artists", 50, 0).await?))
      }),
      self.history_rails()
    );
    let albums = albums?;
    let artists = artists?;
    let favorite_albums = mapped(&albums, "albums", items::album_card);
    let favorite_artists = mapped(&artists, "artists", items::artist_card);
    let recent: std::collections::HashSet<_> = history
      .iter()
      .find(|s| s["id"] == "recent_albums")
      .and_then(|s| s["items"].as_array())
      .into_iter()
      .flatten()
      .filter_map(|a| a["ref"].as_str().map(str::to_owned))
      .collect();
    let mut out = vec![
      shelf(
        "mixes",
        "Qobuz Mixes",
        "playlist",
        [("daily", "DailyQ"), ("weekly", "WeeklyQ"), ("fav", "FavQ"), ("top", "TopQ")]
          .into_iter()
          .map(|(id, title)| {
            let colors: [&str; 3] = match id {
              "daily" => ["#221725", "#d17655", "#f8dda7"],
              "weekly" => ["#281a64", "#bc5fce", "#f4aac4"],
              "fav" => ["#582140", "#dc725d", "#ffd4a0"],
              _ => ["#143744", "#4ba6a0", "#d2c17a"],
            };
            json!({"kind":"playlist","ref":format!("mix:{id}"),"title":title,"art_colors":colors})
          })
          .collect(),
        None,
      ),
      shelf(
        "favorite_albums",
        "Library albums",
        "album",
        favorite_albums.iter().take(18).cloned().collect(),
        None,
      ),
      shelf(
        "top_artists",
        "Your top artists",
        "artist",
        favorite_artists.iter().take(18).cloned().collect(),
        None,
      ),
    ];
    out.push(shelf(
      "rediscover",
      "Rediscover your library",
      "album",
      favorite_albums
        .iter()
        .filter(|a| !recent.contains(a["ref"].as_str().unwrap_or_default()))
        .take(18)
        .cloned()
        .collect(),
      None,
    ));
    out.extend(history);
    out.push(shelf("pinned", "Pinned", "album", self.load_pins().await, None));
    match releases {
            Ok(releases) => out.push(shelf("release_watch", "Release watch", "album", mapped(&releases, "items", items::album_card), Some("releases:0".into()))),
            Err(_) => out.push(json!({"id":"release_watch","title":"Release watch","error":"Release watch could not be loaded. Retry this page."})),
        }
    let recent_albums = out
      .iter()
      .find(|s| s["id"] == "recent_albums")
      .and_then(|s| s["items"].as_array())
      .cloned()
      .unwrap_or_default();
    let mut radio_seen = std::collections::HashSet::new();
    let radio = recent_albums
      .iter()
      .chain(favorite_albums.iter())
      .filter(|a| radio_seen.insert(a["ref"].clone().to_string()))
      .take(12)
      .map(|album| {
        let mut card = album.clone();
        card["kind"] = json!("playlist");
        card["ref"] = json!(format!("radio:{}", album["ref"].as_str().unwrap_or_default()));
        card
      })
      .collect();
    out.push(shelf("radio", "Radio stations", "playlist", radio, None));
    if !deep {
      return Ok(out);
    }
    let favorites_raw = items::collection(albums.get("albums"));
    let mut genre_counts = std::collections::HashMap::<u64, usize>::new();
    for album in &favorites_raw {
      if let Some(id) = album.pointer("/genre/id").and_then(Value::as_u64) {
        *genre_counts.entry(id).or_default() += 1;
      }
    }
    let mut genres: Vec<_> = genre_counts.into_iter().collect();
    genres.sort_by_key(|(id, n)| (std::cmp::Reverse(*n), *id));
    let mut essentials = vec![];
    let mut genre_jobs = tokio::task::JoinSet::new();
    for (position, (id, _)) in genres.into_iter().take(3).enumerate() {
      let client = self.session.api.clone();
      genre_jobs.spawn(async move {
        (
          position,
          client.get_discover_albums("/discover/idealDiscography", Some(vec![id]), 0, 6).await,
        )
      });
    }
    let mut genre_rows = vec![];
    while let Some(result) = genre_jobs.join_next().await {
      if let Ok((position, Ok(found))) = result {
        genre_rows.push((position, mapped(&serialized(found), "items", items::album_card)));
      }
    }
    genre_rows.sort_by_key(|(position, _)| *position);
    for (_, rows) in genre_rows {
      essentials.extend(rows);
    }
    out.push(shelf("essentials", "Essentials by genre", "album", essentials, None));
    if let Some(seed) = recent_albums.first().or(favorite_albums.first()) {
      if let Some(id) = seed["ref"].as_str().and_then(|r| r.strip_prefix("album:")) {
        if let Ok(found) = self.session.api.get_album_suggest(id).await {
          out.push(shelf(
            "similar",
            &format!("Similar to {}", seed["title"].as_str().unwrap_or_default()),
            "album",
            mapped(&serialized(found), "albums", items::album_card),
            None,
          ));
        }
      }
    }
    let mut seen: std::collections::HashSet<_> =
      favorite_artists.iter().filter_map(|a| a["ref"].as_str().map(str::to_owned)).collect();
    let mut follow = vec![];
    let mut follow_jobs = tokio::task::JoinSet::new();
    for (position, seed) in favorite_artists.iter().take(4).enumerate() {
      if let Some(id) =
        seed["ref"].as_str().and_then(|s| s.strip_prefix("artist:")).and_then(|s| s.parse().ok())
      {
        let client = self.session.api.clone();
        follow_jobs.spawn(async move { (position, client.get_similar_artists(id, 12, 0).await) });
      }
    }
    let mut follow_rows = vec![];
    while let Some(result) = follow_jobs.join_next().await {
      if let Ok((position, Ok(found))) = result {
        follow_rows.push((position, mapped(&serialized(found), "items", items::artist_card)));
      }
    }
    follow_rows.sort_by_key(|(position, _)| *position);
    for (_, rows) in follow_rows {
      for item in rows {
        if seen.insert(item["ref"].as_str().unwrap_or_default().to_owned()) {
          follow.push(item);
        }
      }
    }
    follow.truncate(18);
    out.push(shelf("follow", "Artists to follow", "artist", follow, None));
    if let Some(id) = spotlight_artist(&favorite_artists) {
      if let Ok(page) = self.session.api.get_artist_page(id, None).await {
        let page = serialized(page);
        let mut header = items::artist_card(&page);
        header["description"] = page.pointer("/biography/content").cloned().unwrap_or_default();
        let mut albums = vec![];
        let mut seen = std::collections::HashSet::new();
        for kind in ["album", "live", "epSingle", "compilation"] {
          for group in
            page["releases"].as_array().into_iter().flatten().filter(|g| g["type"] == kind)
          {
            for album in mapped(group, "items", items::album_card) {
              if seen.insert(album["ref"].to_string()) {
                albums.push(album);
              }
            }
          }
        }
        albums.truncate(6);
        let mut spotlight =
          shelf("spotlight", "Artist spotlight", "album", albums, Some(format!("artist:{id}")));
        spotlight["header"] = header;
        spotlight["extra_tracks"] =
          json!(mapped(&page, "top_tracks", |v| items::track_card(v, None)));
        out.push(spotlight);
      }
    }
    Ok(out)
  }
}

#[cfg(test)]
#[path = "checks/browsing_tests.rs"]
mod checks;
