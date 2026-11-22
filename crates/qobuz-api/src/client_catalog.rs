use super::client::*;

impl QobuzClient {
  // === Get endpoints ===

  /// Get album by ID
  pub async fn get_album(&self, album_id: &str) -> Result<Album> {
    let url = endpoints::build_url(paths::ALBUM_GET);
    let http_response =
      self.signed_get(&url, "albumget", &[("album_id", album_id.to_string())]).await?;
    let status = http_response.status();
    log::debug!("[API] get_album({}) status={}", album_id, status);

    if status == StatusCode::NOT_FOUND {
      log::warn!("[API] get_album({}) returned 404 — album not found", album_id);
      return Err(ApiError::ApiResponse(format!("Album {} not found (404)", album_id)));
    }
    if !status.is_success() {
      log::error!("[API] get_album({}) unexpected status={}", album_id, status);
      return Err(ApiError::ApiResponse(format!("get_album({}) status {}", album_id, status)));
    }

    let response: Value = http_response.json().await?;
    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get featured albums by type (new-releases, press-awards, most-streamed)
  pub async fn get_featured_albums(
    &self,
    featured_type: &str,
    limit: u32,
    offset: u32,
    genre_id: Option<u64>,
  ) -> Result<SearchResultsPage<Album>> {
    let url = endpoints::build_url(paths::ALBUM_GET_FEATURED);
    let mut query = vec![
      ("type".to_string(), featured_type.to_string()),
      ("limit".to_string(), limit.to_string()),
      ("offset".to_string(), offset.to_string()),
    ];

    if let Some(gid) = genre_id {
      query.push(("genre_id".to_string(), gid.to_string()));
    }

    let params: Vec<(&str, String)> = query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let http_response = self.signed_get(&url, "albumgetFeatured", &params).await?;
    log::debug!("[API] get_featured_albums({}) status={}", featured_type, http_response.status());
    let response: Value = http_response.json().await?;

    let albums = response
      .get("albums")
      .ok_or_else(|| ApiError::ApiResponse("No albums in response".to_string()))?;

    Ok(qobuz_models::artist_display::from_catalog_value(albums.clone())?)
  }

  /// Get Release Watch — new releases from artists, labels or awards the
  /// user follows. Qobuz mobile surfaces this as "Radar de Novedades" /
  /// "Release Watch". Endpoint and signature confirmed in
  /// `qbz-nix-docs/qobuz-api-inferred-openapi-v9.7.0.3.yaml`.
  ///
  /// `release_type` is required and must be one of `artists`, `labels`,
  /// `awards` (matches the three tabs in the mobile UI).
  pub async fn get_release_watch(
    &self,
    release_type: &str,
    limit: u32,
    offset: u32,
  ) -> Result<SearchResultsPage<Album>> {
    let url = endpoints::build_url(paths::FAVORITE_GET_NEW_RELEASES);
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();

    let http_response = self
      .http()?
      .get(&url)
      .headers(self.authenticated_headers().await?)
      .query(&[
        ("type", release_type),
        ("limit", limit_str.as_str()),
        ("offset", offset_str.as_str()),
      ])
      .send()
      .await?;

    let status = http_response.status();
    log::info!(
      "[API] get_release_watch(type={}, limit={}, offset={}) status={}",
      release_type,
      limit,
      offset,
      status
    );

    let body_text = http_response.text().await?;
    if !status.is_success() {
      log::warn!(
        "[API] get_release_watch non-success body (first 400 chars): {}",
        body_text.chars().take(400).collect::<String>()
      );
      return Err(ApiError::ApiResponse(format!("get_release_watch status {}", status)));
    }

    let response: Value = serde_json::from_str(&body_text)?;

    // Shape is V2GenericListDto<AlbumDto> per x20/b.java — a thin
    // pagination envelope: {has_more: bool, items: [...]}. No total/
    // offset/limit fields are returned. We only need items for the UI
    // and the caller already knows the offset/limit it asked for, so
    // we project onto SearchResultsPage<Album> to stay compatible with
    // the rest of the album-list plumbing.
    //
    // Note on shape: AlbumDto carries both `artist` (singular, legacy)
    // and `artists` (array, current). `/favorite/getNewReleases` tends
    // to omit `artist` and only populate `artists[]`, which made our
    // `Album` struct deserialize an empty `artist` and the UI show
    // "Unknown Artist". Backfill `artist` from `artists[0]` before
    // parsing so the existing deserializer just works.
    let mut items_value = response.get("items").cloned().unwrap_or(Value::Null);
    if let Some(items_arr) = items_value.as_array_mut() {
      for item in items_arr {
        if let Some(obj) = item.as_object_mut() {
          let needs_backfill = obj.get("artist").map(|v| v.is_null()).unwrap_or(true);
          if needs_backfill {
            if let Some(first_artist) =
              obj.get("artists").and_then(|a| a.as_array()).and_then(|arr| arr.first()).cloned()
            {
              obj.insert("artist".to_string(), first_artist);
            }
          }
        }
      }
    }
    let items: Vec<Album> =
      qobuz_models::artist_display::from_catalog_value(items_value).map_err(|e| {
        log::warn!(
          "[API] get_release_watch items parse error: {}. Body (first 600 chars): {}",
          e,
          body_text.chars().take(600).collect::<String>()
        );
        ApiError::ApiResponse(format!("get_release_watch items parse: {}", e))
      })?;

    let has_more = response.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false);

    let total_hint = if has_more {
      // We don't know the real total; signal "at least one more page"
      // by bumping past the items we have.
      offset + items.len() as u32 + 1
    } else {
      offset + items.len() as u32
    };

    Ok(SearchResultsPage::<Album> { items, total: total_hint, offset, limit })
  }

  /// Albums similar to a seed album (`/album/suggest`). Ported from
  /// the legacy api client.
  pub async fn get_album_suggest(&self, album_id: &str) -> Result<AlbumSuggestResponse> {
    let url = endpoints::build_url(paths::ALBUM_SUGGEST);
    let http_response = self
      .http()?
      .get(&url)
      .headers(self.api_headers().await?)
      .query(&[("album_id", album_id)])
      .send()
      .await?;
    let status = http_response.status();
    if !status.is_success() {
      return Err(ApiError::ApiResponse(format!("get_album_suggest({album_id}) status {status}")));
    }
    let response: Value = http_response.json().await?;
    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Dynamic suggestions for mixes (`POST /dynamic/suggest`). Seeds
  /// from recently-listened track ids. Returns the suggested tracks
  /// (items parsed leniently). Ported from the legacy api client;
  /// the `track_to_analysed` payload is optional and omitted here.
  pub async fn get_dynamic_suggest(
    &self,
    listened_track_ids: &[u64],
    limit: u32,
  ) -> Result<Vec<Track>> {
    self.get_dynamic_suggest_full(listened_track_ids, &[], limit).await
  }

  /// Like [`get_dynamic_suggest`] but carrying the `track_to_analysed`
  /// payload — the PRIMARY DailyQ/WeeklyQ path. Tauri seeds this with up to 9
  /// resolved `{track_id, artist_id, genre_id, label_id}` tuples and only
  /// falls back to an empty analysis when a call returns zero items.
  pub async fn get_dynamic_suggest_full(
    &self,
    listened_track_ids: &[u64],
    tracks_to_analyse: &[TrackToAnalyse],
    limit: u32,
  ) -> Result<Vec<Track>> {
    let url = endpoints::build_url(paths::DYNAMIC_SUGGEST);
    let body = serde_json::json!({
        "limit": limit,
        "listened_tracks_ids": listened_track_ids,
        "track_to_analysed": tracks_to_analyse,
    });
    let http_response = self
      .http()?
      .post(&url)
      .headers(self.authenticated_headers().await?)
      .json(&body)
      .send()
      .await?;
    let status = http_response.status();
    if !status.is_success() {
      return Err(ApiError::ApiResponse(format!("get_dynamic_suggest status {status}")));
    }
    let response: Value = http_response.json().await?;
    Ok(qobuz_models::lenient::parse_items_array(&response, "tracks", "dynamic-suggest track"))
  }

  /// Qobuz radio for an artist (`/radio/artist`) — a generated track
  /// list. Ported from the legacy api client.
  pub async fn get_radio_artist(&self, artist_id: &str) -> Result<RadioResponse> {
    self.get_radio(paths::RADIO_ARTIST, "artist_id", artist_id).await
  }

  /// Qobuz radio for a track (`/radio/track`).
  pub async fn get_radio_track(&self, track_id: &str) -> Result<RadioResponse> {
    self.get_radio(paths::RADIO_TRACK, "track_id", track_id).await
  }

  /// Qobuz radio for an album (`/radio/album`).
  pub async fn get_radio_album(&self, album_id: &str) -> Result<RadioResponse> {
    self.get_radio(paths::RADIO_ALBUM, "album_id", album_id).await
  }

  pub(crate) async fn get_radio(&self, path: &str, key: &str, id: &str) -> Result<RadioResponse> {
    let url = endpoints::build_url(path);
    let http_response = self
      .http()?
      .get(&url)
      .headers(self.authenticated_headers().await?)
      .query(&[(key, id)])
      .send()
      .await?;
    let status = http_response.status();
    if !status.is_success() {
      return Err(ApiError::ApiResponse(format!("get_radio({path}, {id}) status {status}")));
    }
    let response: Value = http_response.json().await?;
    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get list of genres
  pub async fn get_genres(&self, parent_id: Option<u64>) -> Result<Vec<GenreInfo>> {
    let url = endpoints::build_url(paths::GENRE_LIST);
    // Force English for consistent genre names across all user regions
    let mut query: Vec<(&str, String)> = vec![("lang", "en".to_string())];

    if let Some(pid) = parent_id {
      query.push(("parent_id", pid.to_string()));
    }

    let http_response = self
      .signed_get(
        &url,
        "genrelist",
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?;
    log::debug!("[API] get_genres(parent={:?}) status={}", parent_id, http_response.status());
    let response: Value = http_response.json().await?;

    let genres = response
      .get("genres")
      .and_then(|g| g.get("items"))
      .ok_or_else(|| ApiError::ApiResponse("No genres in response".to_string()))?;

    Ok(qobuz_models::artist_display::from_catalog_value(genres.clone())?)
  }

  /// Get discover index (home page content: playlists, ideal discography, etc.)
  pub async fn get_discover_index(&self, genre_ids: Option<Vec<u64>>) -> Result<DiscoverResponse> {
    let url = endpoints::build_url(paths::DISCOVER_INDEX);
    let mut query: Vec<(&str, String)> = vec![];

    // Add genre_ids as comma-separated list if provided
    if let Some(gids) = genre_ids {
      if !gids.is_empty() {
        let ids_str = gids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        query.push(("genre_ids", ids_str));
      }
    }

    let http_response = self
      .signed_get_auth(
        &url,
        "discoverindex",
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?;
    log::info!("[API] get_discover_index genre_ids={:?} status={}", query, http_response.status());
    let response: Value = http_response.json().await?;

    // Debug: log the response structure
    if let Some(obj) = response.as_object() {
      log::info!("Discover API response keys: {:?}", obj.keys().collect::<Vec<_>>());
      if let Some(err) = obj.get("message") {
        log::error!("Discover API error: {:?}", err);
      }
      if let Some(code) = obj.get("code") {
        log::error!("Discover API error code: {:?}", code);
      }
    }

    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get discover albums from a specific browse endpoint (newReleases, idealDiscography, mostStreamed)
  pub async fn get_discover_albums(
    &self,
    endpoint: &str,
    genre_ids: Option<Vec<u64>>,
    offset: u32,
    limit: u32,
  ) -> Result<DiscoverData<DiscoverAlbum>> {
    let url = endpoints::build_url(endpoint);
    let mut query: Vec<(&str, String)> = vec![];

    if let Some(gids) = genre_ids {
      if !gids.is_empty() {
        let ids_str = gids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        query.push(("genre_ids", ids_str));
      }
    }

    query.push(("offset", offset.to_string()));
    query.push(("limit", limit.to_string()));

    // Derive method name from endpoint path: "/discover/newReleases" -> "discovernewReleases"
    let method_name = endpoint.replace('/', "").replace('.', "");
    let http_response = self
      .signed_get_auth(
        &url,
        &method_name,
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?;
    log::info!(
      "[API] get_discover_albums({}) query={:?} status={}",
      endpoint,
      query,
      http_response.status()
    );
    let response: serde_json::Value = http_response.json().await?;

    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get discover playlists with optional tag and genre filters
  /// Example: tags=label, genre_ids=112,119
  pub async fn get_discover_playlists(
    &self,
    tag: Option<String>,
    genre_ids: Option<Vec<u64>>,
    limit: Option<u32>,
    offset: Option<u32>,
  ) -> Result<DiscoverPlaylistsResponse> {
    let url = endpoints::build_url(paths::DISCOVER_PLAYLISTS);
    let mut query: Vec<(&str, String)> = vec![];

    // Add tag filter if provided (e.g., "label", "partner")
    if let Some(ref t) = tag {
      query.push(("tags", t.clone()));
    }

    // Add genre_ids as comma-separated list if provided
    if let Some(gids) = genre_ids {
      if !gids.is_empty() {
        let ids_str = gids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        query.push(("genre_ids", ids_str));
      }
    }

    // Add limit (default 20)
    let lim = limit.unwrap_or(20);
    query.push(("limit", lim.to_string()));

    // Add offset (default 0)
    let off = offset.unwrap_or(0);
    query.push(("offset", off.to_string()));

    log::debug!("[API] get_discover_playlists URL: {} query: {:?}", url, query);

    // First get raw JSON to debug structure
    let raw_response: serde_json::Value = self
      .signed_get_auth(
        &url,
        "discoverplaylists",
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?
      .json()
      .await?;

    log::debug!(
      "[API] get_discover_playlists raw response keys: {:?}",
      raw_response.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );

    // Try to parse as expected structure
    let response: DiscoverPlaylistsResponse =
      qobuz_models::artist_display::from_catalog_value(raw_response.clone()).map_err(|e| {
        log::error!("[API] Failed to parse discover playlists response: {}", e);
        log::error!(
          "[API] Raw response: {}",
          serde_json::to_string_pretty(&raw_response).unwrap_or_default()
        );
        e
      })?;

    log::debug!("[API] get_discover_playlists response: {} playlists", response.items.len());

    Ok(response)
  }

  /// Get playlist tags with localized names
  pub async fn get_playlist_tags(&self) -> Result<Vec<PlaylistTag>> {
    let url = endpoints::build_url(paths::PLAYLIST_GET_TAGS);

    let http_response = self.signed_get_auth(&url, "playlistgetTags", &[]).await?;
    log::info!("[API] get_playlist_tags status={}", http_response.status());

    let raw: PlaylistTagsResponse = http_response.json().await?;

    // Get current locale (e.g., "en", "es", "fr", "de")
    let locale = self.locale().await;
    let lang = locale.split('-').next().unwrap_or("en");

    // Convert raw tags to PlaylistTag with localized name
    let tags: Vec<PlaylistTag> = raw
      .tags
      .into_iter()
      .filter(|tag| tag.is_discover.as_deref() == Some("true"))
      .filter_map(|tag| {
        // Parse name_json to get localized name
        let name_map: serde_json::Value = serde_json::from_str(&tag.name_json).ok()?;
        let name = name_map
          .get(lang)
          .or_else(|| name_map.get("en"))
          .and_then(|v| v.as_str())
          .map(|s| s.to_string())?;
        let id = tag.featured_tag_id.as_ref().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        Some(PlaylistTag { id, slug: tag.slug, name })
      })
      .collect();

    log::debug!("[API] get_playlist_tags: {} tags", tags.len());
    Ok(tags)
  }

  /// Get track by ID
  pub async fn get_track(&self, track_id: u64) -> Result<Track> {
    let url = endpoints::build_url(paths::TRACK_GET);
    let http_response =
      self.signed_get(&url, "trackget", &[("track_id", track_id.to_string())]).await?;
    let status = http_response.status();
    log::debug!("[API] get_track({}) status={}", track_id, status);

    if status == StatusCode::NOT_FOUND {
      log::warn!("[API] get_track({}) returned 404 — track no longer available", track_id);
      return Err(ApiError::TrackUnavailable(track_id));
    }
    if !status.is_success() {
      log::error!("[API] get_track({}) unexpected status={}", track_id, status);
      return Err(ApiError::ApiResponse(format!("get_track({}) status {}", track_id, status)));
    }

    let response: Value = http_response.json().await?;
    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }
}
