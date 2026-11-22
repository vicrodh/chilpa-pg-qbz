use crate::{
  catalog::{CatalogService, RpcFault, RpcResult},
  projection as items,
};
use serde_json::{json, Value};
use std::time::Duration;

pub(crate) fn shelf(
  id: &str,
  title: &str,
  kind: &str,
  items: Vec<Value>,
  more: Option<String>,
) -> Value {
  json!({"id":id,"title":title,"kind":kind,"items":items,"more":more})
}
pub(crate) fn mapped(value: &Value, field: &str, map: fn(&Value) -> Value) -> Vec<Value> {
  items::collection(value.get(field)).iter().map(map).collect()
}
pub(crate) fn serialized(value: impl serde::Serialize) -> Value {
  serde_json::to_value(value).unwrap_or_default()
}
fn module_digest(value: &str) -> u64 {
  value
    .bytes()
    .fold(0xcbf29ce484222325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3))
}
fn label_module_id(container: &Value, rows: &[Value]) -> String {
  if let Some(id) = container["id"].as_str().map(str::trim).filter(|id| !id.is_empty()) {
    return format!(
      "releases:{}",
      url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
    );
  }
  if let Some(title) = container["title"].as_str().map(str::trim).filter(|title| !title.is_empty())
  {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    return format!("releases:title:{:016x}", module_digest(&title));
  }
  // Anonymous modules use an order-independent membership fingerprint with fixed scratch space.
  let (mut count, mut combined, mut sum) = (0_u64, 0_u64, 0_u64);
  for reference in rows.iter().filter_map(|row| row["ref"].as_str()) {
    let digest = module_digest(reference);
    count = count.wrapping_add(1);
    combined ^= digest;
    sum = sum.wrapping_add(digest);
  }
  format!("releases:items:{count:x}-{combined:016x}-{sum:016x}")
}
fn label_release_shelves(id: u64, page: &Value) -> Vec<Value> {
  let mut shelves: Vec<Value> = vec![];
  for container in page["releases"].as_array().into_iter().flatten() {
    let name = container["title"].as_str().or(container["id"].as_str()).unwrap_or("Releases");
    let kind = if name.to_ascii_lowercase().contains("next") {
      "upcoming"
    } else if name.to_ascii_lowercase().contains("award") {
      "awards"
    } else {
      "albums"
    };
    let rows: Vec<_> =
      items::collection(container.get("data")).iter().map(items::album_card).collect();
    let key = label_module_id(container, &rows);
    if shelves.iter().any(|row| row["id"] == key) {
      continue;
    }
    shelves.push(shelf(
      &key,
      release_title(name),
      "album",
      rows,
      Some(format!("label:{id}:{kind}:0")),
    ));
  }
  shelves
}
fn award_route<'a>(parts: &'a [&str]) -> Result<(&'a str, Option<u32>), RpcFault> {
  let id = parts
    .get(1)
    .copied()
    .filter(|id| !id.trim().is_empty())
    .ok_or_else(|| RpcFault::invalid("award"))?;
  let offset = match parts {
    ["award", _] => None,
    ["award", _, "albums", offset] => Some(
      offset
        .parse::<u32>()
        .ok()
        .filter(|offset| offset.checked_add(101).is_some())
        .ok_or_else(|| RpcFault::invalid("award offset"))?,
    ),
    _ => return Err(RpcFault::invalid("award section")),
  };
  Ok((id, offset))
}
fn award_header(id: &str, page: &Value) -> Result<Value, RpcFault> {
  let title = page["name"]
    .as_str()
    .map(str::trim)
    .filter(|title| !title.is_empty())
    .ok_or_else(|| RpcFault::new(-32000, "Award metadata is unavailable"))?;
  if page["id"].as_str().is_some_and(|returned| returned != id) {
    return Err(RpcFault::new(-32000, "Award metadata does not match the requested award"));
  }
  let mut header = json!({
    "kind":"award","ref":format!("award:{id}"),"title":title,
    "image":items::image(page).or_else(|| items::image(&page["magazine"])),
    "subtitle":page["magazine"]["name"],"awarded_at":page["awarded_at"],
  });
  items::image_fallback(&mut header);
  Ok(header)
}
fn award_albums_page(id: &str, offset: u32, page: &Value) -> RpcResult {
  let raw = page["items"]
    .as_array()
    .ok_or_else(|| RpcFault::new(-32000, "Award releases are unavailable"))?;
  let total = page["total"]
    .as_u64()
    .ok_or_else(|| RpcFault::new(-32000, "Award release count is unavailable"))?;
  let consumed = u64::from(offset) + raw.len() as u64;
  if page["offset"].as_u64() != Some(u64::from(offset))
    || raw.len() > 100
    || total < consumed
    || (raw.is_empty() && total > consumed)
  {
    return Err(RpcFault::new(-32000, "Award releases did not advance"));
  }
  let rows: Vec<_> = raw.iter().map(items::album_card).collect();
  Ok(json!({
    "title":"All releases","items":rows,
    "tabs":[{"title":"All releases","ref":format!("award:{id}:albums:0")}],
    "next":(consumed < total).then(|| format!("award:{id}:albums:{consumed}")),
  }))
}
impl CatalogService {
  pub(crate) async fn load_page(&self, request: Value) -> RpcResult {
    self.require_session().await?;
    if request["ref"].as_str().is_some_and(|reference| reference.starts_with("library:")) {
      return self.library.page(&self.session, request).await;
    }
    if request["refresh"] == true {
      self.invalidate_catalog();
    }
    let reference = request["ref"].as_str().unwrap_or("discover:home");
    self.memoized(&format!("page {reference}"), Duration::from_secs(300), || async {
            let parts: Vec<_> = reference.split(':').collect();
            match parts.first().copied().unwrap_or_default() {
                "discover" => self.load_discovery(&parts).await,
                "browse" => self.browse_discovery(&parts).await,
                "artist" => self.artist_navigation(&parts).await,
                "album" => {
                    let id = parts.get(1).copied().ok_or_else(|| RpcFault::invalid("album"))?;
                    let (album, suggestions) = tokio::join!(self.load_album(json!({"ref":reference})), self.session.api.get_album_suggest(id));
                    let mut album = album?["album"].clone();
                    let related = mapped(&serialized(suggestions?), "albums", items::album_card);
                    let mut shelves = vec![];
                    if let Some(artist) = album["artist_ref"].as_str().and_then(|r| r.strip_prefix("artist:")).and_then(|s| s.parse().ok()) {
                        if let Ok(page) = self.artist_releases(artist).await {
                            let mut seen = std::collections::HashSet::new();
                            let peers: Vec<_> = page["releases"].as_array().into_iter().flatten().flat_map(|g| mapped(g,"items",items::album_card)).filter(|a| a["ref"] != reference && seen.insert(a["ref"].to_string())).take(18).collect();
                            shelves.push(shelf("artist", "From the same artist", "album", peers, Some(format!("artist:{artist}"))));
                        }
                    }
                    shelves.push(shelf("related", "You may also like", "album", related, None));
                    shelves.push(shelf("radio", "Radio", "playlist", vec![json!({"kind":"playlist","ref":format!("radio:album:{id}"),"title":format!("{} Radio",album["title"].as_str().unwrap_or_default()),"image":album["image"]})], None));
                    album["shelves"] = json!(shelves);
                    Ok(album)
                },
                "releases" => {
                    let offset = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let page = serialized(self.session.api.get_release_watch("artists",100,offset).await?);
                    let rows = mapped(&page,"items",items::album_card);
                    Ok(json!({"title":"Release watch","next":(rows.len()==100).then(|| format!("releases:{}",offset+100)),"items":rows}))
                },
                "label" => self.label_navigation(&parts).await,
                "award" => self.award_navigation(&parts).await,
                _ => Err(RpcFault::invalid("page reference")),
            }
        }).await
  }
  async fn artist_releases(&self, id: u64) -> RpcResult {
    self
      .memoized(&format!("artist-page {id}"), Duration::from_secs(300), || async {
        Ok(serialized(self.session.api.get_artist_page(id, None).await?))
      })
      .await
  }
  async fn artist_navigation(&self, parts: &[&str]) -> RpcResult {
    let id: u64 =
      parts.get(1).and_then(|s| s.parse().ok()).ok_or_else(|| RpcFault::invalid("artist"))?;
    if let Some(kind) = parts.get(2) {
      let offset = parts.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
      let page = serialized(self.session.api.get_releases_grid(id, kind, 100, offset, None).await?);
      let rows = mapped(&page, "items", items::album_card);
      let next = (page["has_more"] == true && !rows.is_empty())
        .then(|| format!("artist:{id}:{kind}:{}", offset + rows.len() as u32));
      return Ok(json!({"title":release_title(kind),"items":rows,"next":next}));
    }
    let page = self.artist_releases(id).await?;
    let mut artist = items::artist_card(&page);
    if artist["image"].as_str().is_none() {
      if let Ok(detail) = self.session.api.get_artist(id, false).await {
        if let Some(image) = items::image(&serialized(detail)) {
          artist["image"] = image.into();
        }
      }
    }
    artist["description_source"] = json!(page
      .pointer("/biography/source")
      .and_then(|v| v.as_str().or_else(|| v["name"].as_str()))
      .unwrap_or("Qobuz"));
    artist["description"] = page.pointer("/biography/content").cloned().unwrap_or(Value::Null);
    if let Some(category) = page["artist_category"].as_str().filter(|c| !c.is_empty()) {
      let mut chars = category.chars();
      let role: String = chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default();
      artist["roles"] = json!([role]);
    }
    let mut shelves = vec![];
    if page["last_release"].is_object() {
      shelves.push(shelf(
        "latest",
        "Latest release",
        "album",
        vec![items::album_card(&page["last_release"])],
        None,
      ));
    }
    let mut groups = std::collections::BTreeMap::<String, Vec<Value>>::new();
    let mut unfinished = std::collections::HashSet::<String>::new();
    let mut seen = std::collections::HashSet::new();
    let mut labels = std::collections::BTreeMap::new();
    for group in page["releases"].as_array().into_iter().flatten() {
      let kind = group["type"].as_str().unwrap_or("other");
      if group["has_more"] == true {
        unfinished.insert(kind.into());
      }
      for raw in items::collection(group.get("items")) {
        if !seen.insert(raw["id"].to_string()) {
          continue;
        }
        if kind == "album" && raw["label"]["id"].is_number() {
          labels.insert(raw["label"]["id"].to_string(), items::label_card(&raw["label"]));
        }
        if kind != "download" {
          groups.entry(kind.into()).or_default().push(items::album_card(&raw));
        }
      }
    }
    let more = |kind: &str, rows: &[Value]| {
      unfinished.contains(kind).then(|| format!("artist:{id}:{kind}:{}", rows.len()))
    };
    for kind in [
      "album",
      "epSingle",
      "ep",
      "single",
      "live",
      "compilation",
      "composer",
      "other",
      "awardedRelease",
      "next",
    ] {
      if let Some(rows) = groups.remove(kind) {
        let next = more(kind, &rows);
        shelves.push(shelf(kind, release_title(kind), "album", rows, next));
      }
    }
    for (kind, rows) in groups {
      let next = more(&kind, &rows);
      shelves.push(shelf(&kind, release_title(&kind), "album", rows, next));
    }
    if !labels.is_empty() {
      shelves.push(shelf("labels", "Labels", "label", labels.into_values().collect(), None));
    }
    let tracks = mapped(&page, "top_tracks", |v| items::track_card(v, None));
    shelves.push(shelf("tracks", "Top tracks", "track", tracks, None));
    shelves.push(shelf("radio", "Artist radio", "playlist", vec![json!({"kind":"playlist","ref":format!("radio:artist:{id}"),"title":format!("{} Radio",artist["title"].as_str().unwrap_or_default()),"image":artist["image"]})], None));
    shelves.push(shelf(
      "appearances",
      "Appears on",
      "track",
      mapped(&page, "tracks_appears_on", |v| items::track_card(v, None)),
      None,
    ));
    shelves.push(shelf(
      "similar",
      "Similar artists",
      "artist",
      mapped(&page, "similar_artists", items::artist_card),
      None,
    ));
    shelves.push(shelf(
      "playlists",
      "Playlists",
      "playlist",
      mapped(&page, "playlists", items::playlist_card),
      None,
    ));
    if let Ok(stories) = self.session.api.get_artist_story(id, 0, 2).await {
      let rows: Vec<Value> = stories.items.iter().map(|story| json!({
                "kind": "link",
                "ref": format!("story:{}", story.id),
                "title": story.title,
                "author": story.authors.as_ref().and_then(|a| a.first()).map(|a| a.name.clone()).unwrap_or_default(),
                "image": story.image.clone().or_else(|| story.images.as_ref().and_then(|i| i.first()).map(|i| i.url.clone())).unwrap_or_default(),
                "url": format!("https://play.qobuz.com/magazine/story/{}", story.id),
            })).collect();
      if !rows.is_empty() {
        shelves.push(shelf("stories", "Magazine", "link", rows, None));
      }
    }
    for section in &mut shelves {
      section["sortable"] = json!(section["kind"] == "album");
    }
    Ok(json!({"title":artist["title"],"header":artist,"shelves":shelves}))
  }
  async fn label_navigation(&self, parts: &[&str]) -> RpcResult {
    let id: u64 =
      parts.get(1).and_then(|s| s.parse().ok()).ok_or_else(|| RpcFault::invalid("label"))?;
    if let Some(kind) = parts.get(2) {
      let offset = parts.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
      let page = match *kind {
        "albums" => serialized(
          self.session.api.get_label_albums(id, 100, offset, None, None, None, None, None).await?,
        ),
        "upcoming" => {
          serialized(self.session.api.get_label_next_releases(id, 100, offset, None).await?)
        }
        "awards" => serialized(
          self.session.api.get_label_awarded_releases(id, 100, offset, None, None, None).await?,
        ),
        "playlists" => serialized(self.session.api.get_label_playlists(id, 100, offset).await?),
        "artists" => serialized(self.session.api.get_label_top_artists(id, 100, offset).await?),
        _ => return Err(RpcFault::invalid("label section")),
      };
      let rows = mapped(
        &page,
        "items",
        match *kind {
          "playlists" => items::playlist_card,
          "artists" => items::artist_card,
          _ => items::album_card,
        },
      );
      return Ok(
        json!({"title":kind,"items":rows,"next":(page["has_more"].as_bool().unwrap_or(rows.len()==100) && !rows.is_empty()).then(|| format!("label:{id}:{kind}:{}", offset+rows.len() as u32))}),
      );
    }
    let page = serialized(self.session.api.get_label_page(id).await?);
    let mut shelves = label_release_shelves(id, &page);
    shelves.push(shelf(
      "tracks",
      "Top tracks",
      "track",
      mapped(&page, "top_tracks", |v| items::track_card(v, None)),
      None,
    ));
    shelves.push(shelf(
      "artists",
      "Top artists",
      "artist",
      mapped(&page, "top_artists", items::artist_card),
      None,
    ));
    shelves.push(shelf(
      "playlists",
      "Playlists",
      "playlist",
      mapped(&page, "playlists", items::playlist_card),
      None,
    ));
    for section in &mut shelves {
      section["sortable"] = json!(section["kind"] == "album");
    }
    let tabs: Vec<_> = [
      ("albums", "All releases"),
      ("upcoming", "Upcoming releases"),
      ("awards", "Awarded releases"),
      ("playlists", "Playlists"),
      ("artists", "Top artists"),
    ]
    .into_iter()
    .map(|(kind, title)| json!({"title":title,"ref":format!("label:{id}:{kind}:0")}))
    .collect();
    Ok(
      json!({"tabs":tabs,"title":page["name"],"header":items::label_card(&page),"shelves":shelves}),
    )
  }
  async fn award_navigation(&self, parts: &[&str]) -> RpcResult {
    let (id, offset) = award_route(parts)?;
    if let Some(offset) = offset {
      let albums = serialized(self.session.api.get_award_albums(id, 100, offset).await?);
      return award_albums_page(id, offset, &albums);
    }
    let (metadata, albums) = tokio::try_join!(
      self.session.api.get_award_page(id),
      self.session.api.get_award_albums(id, 100, 0),
    )?;
    let header = award_header(id, &serialized(metadata))?;
    let mut page = award_albums_page(id, 0, &serialized(albums))?;
    page["title"] = header["title"].clone();
    page["header"] = header;
    Ok(page)
  }
}

fn release_title(kind: &str) -> &str {
  match kind {
    "album" => "Albums",
    "live" => "Live releases",
    "ep-single" | "epSingle" | "ep" | "single" => "EPs and singles",
    "download" => "Downloads",
    "composer" => "Composer",
    "awardedRelease" | "awardedReleases" => "Awarded releases",
    "other" => "Other releases",
    "next" | "nextReleases" => "Upcoming releases",
    "albums" | "allReleases" => "All releases",
    "compilation" => "Compilations",
    "appears-on" => "Appears on",
    other => other,
  }
}

#[cfg(test)]
#[path = "checks/navigation_tests.rs"]
mod checks;
