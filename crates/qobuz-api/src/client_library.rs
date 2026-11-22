use super::client::*;

impl QobuzClient {
  /// Get user favorites (requires auth + signature)
  pub async fn get_favorites(&self, fav_type: &str, limit: u32, offset: u32) -> Result<Value> {
    // Back off before the network if the 403 breaker is open (issue #637).
    self.forbidden_guard()?;
    let url = endpoints::build_url(paths::FAVORITE_GET_USER_FAVORITES);

    let http_response = self
      .http()?
      .get(&url)
      .headers(self.authenticated_headers().await?)
      .query(&[("type", fav_type), ("limit", &limit.to_string()), ("offset", &offset.to_string())])
      .send()
      .await?;
    let status = http_response.status();
    log::debug!("[API] get_favorites({}) status={}", fav_type, status);
    // Feed the breaker AND check status BEFORE decoding: a 403's body is not
    // our JSON envelope (it's an edge/WAF HTML/empty body), so a bare
    // `.json()` surfaced it as a misleading "error decoding response body"
    // instead of the real 403 (issue #637).
    self.note_forbidden_status(status);
    if !status.is_success() {
      let preview = body_preview(http_response).await;
      if status == StatusCode::FORBIDDEN {
        log::warn!("get_favorites({}) 403{}", fav_type, preview);
        return Err(ApiError::Forbidden(preview));
      }
      return Err(ApiError::ApiResponse(format!(
        "get_favorites failed with status {}{}",
        status, preview
      )));
    }
    let response: Value = http_response.json().await?;

    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get user's playlists
  pub async fn get_user_playlists(&self) -> Result<Vec<Playlist>> {
    let url = endpoints::build_url(paths::PLAYLIST_GET_USER_PLAYLISTS);
    let mut out = Vec::new();
    let mut offset = 0_u32;
    let mut seen = std::collections::HashSet::new();
    loop {
      let http_response = self
        .signed_get_auth(
          &url,
          "playlistgetUserPlaylists",
          &[("limit", "500".to_string()), ("offset", offset.to_string())],
        )
        .await?;
      if !http_response.status().is_success() {
        return Err(ApiError::ApiResponse(format!(
          "Playlist listing failed: {}",
          http_response.status()
        )));
      }
      let response: Value = http_response.json().await?;
      let page = &response["playlists"];
      let items: Vec<Playlist> = qobuz_models::artist_display::from_catalog_value(
        page
          .get("items")
          .cloned()
          .ok_or_else(|| ApiError::ApiResponse("No playlists in response".into()))?,
      )?;
      let count = items.len();
      let before = out.len();
      out.extend(items.into_iter().filter(|p| seen.insert(p.id)));
      offset += count as u32;
      if count == 0
        || out.len() == before
        || page["total"].as_u64().is_some_and(|total| u64::from(offset) >= total)
        || (page["total"].is_null() && count < 500)
      {
        break;
      }
    }
    Ok(out)
  }

  /// Search playlists
  pub async fn search_playlists(
    &self,
    query: &str,
    limit: u32,
    offset: u32,
  ) -> Result<SearchResultsPage<Playlist>> {
    let url = endpoints::build_url(paths::PLAYLIST_SEARCH);
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();

    let http_response = self
      .http()?
      .get(&url)
      .headers(self.api_headers().await?)
      .query(&[("query", query), ("limit", &limit_str), ("offset", &offset_str)])
      .send()
      .await?;
    log::debug!("[API] search_playlists status={}", http_response.status());
    let response: Value = http_response.json().await?;

    let playlists = response
      .get("playlists")
      .ok_or_else(|| ApiError::ApiResponse("No playlists in response".to_string()))?;

    Ok(qobuz_models::artist_display::from_catalog_value(playlists.clone())?)
  }

  /// Create a new playlist
  pub async fn create_playlist(
    &self,
    name: &str,
    description: Option<&str>,
    is_public: bool,
  ) -> Result<Playlist> {
    let url = endpoints::build_url(paths::PLAYLIST_CREATE);

    let mut params = vec![("name", name.to_string()), ("is_public", is_public.to_string())];
    if let Some(desc) = description {
      params.push(("description", desc.to_string()));
    }

    let params: Vec<(&str, String)> = params.iter().map(|(k, v)| (*k, v.clone())).collect();
    let response: Playlist =
      self.signed_get_auth(&url, "playlistcreate", &params).await?.json().await?;

    Ok(response)
  }

  /// Delete a playlist
  pub async fn delete_playlist(&self, playlist_id: u64) -> Result<()> {
    let url = endpoints::build_url(paths::PLAYLIST_DELETE);

    self
      .signed_get_auth(&url, "playlistdelete", &[("playlist_id", playlist_id.to_string())])
      .await?;

    Ok(())
  }

  /// Add tracks to a playlist
  pub async fn add_tracks_to_playlist(&self, playlist_id: u64, track_ids: &[u64]) -> Result<()> {
    let url = endpoints::build_url(paths::PLAYLIST_ADD_TRACKS);
    let track_ids_str = track_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");

    self
      .signed_get_auth(
        &url,
        "playlistaddTracks",
        &[("playlist_id", playlist_id.to_string()), ("track_ids", track_ids_str)],
      )
      .await?;

    Ok(())
  }

  /// Remove tracks from a playlist
  pub async fn remove_tracks_from_playlist(
    &self,
    playlist_id: u64,
    playlist_track_ids: &[u64],
  ) -> Result<()> {
    let url = endpoints::build_url(paths::PLAYLIST_DELETE_TRACKS);
    let track_ids_str =
      playlist_track_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");

    self
      .signed_get_auth(
        &url,
        "playlistdeleteTracks",
        &[("playlist_id", playlist_id.to_string()), ("playlist_track_ids", track_ids_str)],
      )
      .await?;

    Ok(())
  }

  /// Move tracks WITHIN a playlist — `/playlist/updateTracksPosition`.
  ///
  /// `playlist_track_ids` are MEMBERSHIP ids, the same id space
  /// [`Self::remove_tracks_from_playlist`] takes — not catalog track ids.
  /// `insert_before` is the 0-based slot the moved rows must land in front
  /// of, counted against the playlist AS IT IS at call time.
  ///
  /// # Provenance, because this one is not in the current spec
  ///
  /// The argument shape is the DESKTOP web-player capture's
  /// (`qbz-nix-docs/qobuz-api-delta-desktop-8.2.0-b015.md:95` —
  /// `{playlist_id, playlist_track_ids, insert_before}`). The decompiled
  /// Android route table lists the same path, and that entry is corroboration
  /// without establishing its request argument types.
  ///
  /// GET + [`Self::signed_get_auth`], exactly like the two neighbours this
  /// call is sequenced with. The capture records a POST, but every playlist
  /// write in this client is a signed GET against api.json/0.2 — including
  /// `addTracks` and `deleteTracks`, whose captures were POSTs too — and the
  /// signature method name follows the same `<namespace><Method>` rule the
  /// neighbours use, hence `playlistupdateTracksPosition`.
  ///
  /// The exact reading of `insert_before` is the ONE part of this verb no
  /// capture in the repo pins down, so the request is logged in full at debug
  /// level and callers must treat a successful-but-wrong move as cosmetic —
  /// never as a reason to abort a repair that already landed.
  pub async fn update_playlist_tracks_position(
    &self,
    playlist_id: u64,
    playlist_track_ids: &[u64],
    insert_before: u32,
  ) -> Result<()> {
    let url = endpoints::build_url(paths::PLAYLIST_UPDATE_TRACKS_POSITION);
    let track_ids_str =
      playlist_track_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
    log::debug!(
      "[API] updateTracksPosition playlist_id={} playlist_track_ids={} insert_before={}",
      playlist_id,
      track_ids_str,
      insert_before
    );

    self
      .signed_get_auth(
        &url,
        "playlistupdateTracksPosition",
        &[
          ("playlist_id", playlist_id.to_string()),
          ("playlist_track_ids", track_ids_str),
          ("insert_before", insert_before.to_string()),
        ],
      )
      .await?;

    Ok(())
  }

  /// Update playlist metadata
  pub async fn update_playlist(
    &self,
    playlist_id: u64,
    name: Option<&str>,
    description: Option<&str>,
    is_public: Option<bool>,
  ) -> Result<Playlist> {
    let url = endpoints::build_url(paths::PLAYLIST_UPDATE);

    let mut params = vec![("playlist_id", playlist_id.to_string())];
    if let Some(n) = name {
      params.push(("name", n.to_string()));
    }
    if let Some(d) = description {
      params.push(("description", d.to_string()));
    }
    if let Some(p) = is_public {
      params.push(("is_public", p.to_string()));
    }

    let params_ref: Vec<(&str, String)> = params.iter().map(|(k, v)| (*k, v.clone())).collect();
    let response: Playlist =
      self.signed_get_auth(&url, "playlistupdate", &params_ref).await?.json().await?;

    Ok(response)
  }

  /// Subscribe to a Qobuz playlist (follow it in the user's library)
  pub async fn subscribe_playlist(&self, playlist_id: u64) -> Result<()> {
    let url = endpoints::build_url(paths::PLAYLIST_SUBSCRIBE);

    let response = self
      .signed_get_auth(&url, "playlistsubscribe", &[("playlist_id", playlist_id.to_string())])
      .await?;

    let status = response.status();
    if !status.is_success() {
      let body = body_preview(response).await;
      return Err(ApiError::ApiResponse(format!(
        "playlist/subscribe failed ({}): {}",
        status, body
      )));
    }

    Ok(())
  }

  /// Unsubscribe from a Qobuz playlist
  pub async fn unsubscribe_playlist(&self, playlist_id: u64) -> Result<()> {
    let url = endpoints::build_url(paths::PLAYLIST_UNSUBSCRIBE);

    let response = self
      .signed_get_auth(&url, "playlistunsubscribe", &[("playlist_id", playlist_id.to_string())])
      .await?;

    let status = response.status();
    if !status.is_success() {
      let body = body_preview(response).await;
      return Err(ApiError::ApiResponse(format!(
        "playlist/unsubscribe failed ({}): {}",
        status, body
      )));
    }

    Ok(())
  }

  /// Add item to favorites
  pub async fn add_favorite(&self, fav_type: &str, item_id: &str) -> Result<()> {
    let url = endpoints::build_url(paths::FAVORITE_CREATE);
    let type_key = format!("{}_ids", fav_type); // album_ids, track_ids, artist_ids

    let response = self
      .signed_get_auth(&url, "favoritecreate", &[(type_key.as_str(), item_id.to_string())])
      .await?;

    if response.status().is_success() {
      Ok(())
    } else {
      Err(ApiError::ApiResponse(format!("Failed to add favorite: {}", response.status())))
    }
  }

  /// Remove item from favorites
  pub async fn remove_favorite(&self, fav_type: &str, item_id: &str) -> Result<()> {
    let url = endpoints::build_url(paths::FAVORITE_DELETE);
    let type_key = format!("{}_ids", fav_type);

    let response = self
      .signed_get_auth(&url, "favoritedelete", &[(type_key.as_str(), item_id.to_string())])
      .await?;

    if response.status().is_success() {
      Ok(())
    } else {
      Err(ApiError::ApiResponse(format!("Failed to remove favorite: {}", response.status())))
    }
  }
}
