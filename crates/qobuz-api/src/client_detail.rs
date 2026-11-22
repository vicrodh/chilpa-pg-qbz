use super::client::*;

impl QobuzClient {
  // === Lyrics (v9.9.0.0-beta delta) ===

  /// Raw step-1 lyrics call: signed `GET /track/lyricsUrl`, returning
  /// `(HTTP status, raw body)`. The signed-GET helpers are private to the
  /// client, so this is the only way diagnostic tooling (e.g.
  /// `examples/lyrics_probe.rs`) can observe the exact wire shape; the
  /// typed [`Self::get_lyrics_url`] builds on it.
  ///
  /// `language` (ISO 639-1, API v10) requests a translation; it is appended
  /// to the signed param list ONLY when `Some` (`sign_request` sorts the
  /// kv pairs alphabetically, so signing needs no change). Pass `None` for
  /// the original-only fetch.
  pub async fn get_lyrics_url_raw(
    &self,
    track_id: u64,
    language: Option<&str>,
  ) -> Result<(u16, String)> {
    let (status, _, body) = self.get_lyrics_url_reply(track_id, language).await?;
    Ok((status, body))
  }

  /// The same call as [`Self::get_lyrics_url_raw`], with the pause a busy
  /// answer asks for: `(HTTP status, Retry-After in seconds, raw body)`.
  /// A caller that must tell "no lyrics" from "could not look" reads the
  /// status itself instead of the typed [`Self::get_lyrics_url`], which
  /// turns every non-200 into a miss.
  pub async fn get_lyrics_url_reply(
    &self,
    track_id: u64,
    language: Option<&str>,
  ) -> Result<(u16, Option<u64>, String)> {
    let url = endpoints::build_url(paths::TRACK_LYRICS_URL);
    // Signing method name per the concatenation convention ("trackget" at
    // get_track, "trackgetFileUrl" in auth.rs): path with slashes removed,
    // case preserved -> "tracklyricsUrl". Verified against the live API
    // 2026-06-10 (HTTP 200 with a valid envelope).
    let mut params = vec![("track_id", track_id.to_string())];
    if let Some(language) = language {
      params.push(("language", language.to_string()));
    }
    let response = self.signed_get_auth(&url, "tracklyricsUrl", &params).await?;
    let status = response.status().as_u16();
    let retry_after = response
      .headers()
      .get(reqwest::header::RETRY_AFTER)
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.trim().parse::<u64>().ok());
    let body = response.text().await?;
    Ok((status, retry_after, body))
  }

  /// Step 1: fetch the lyrics-URL envelope for a track.
  ///
  /// `Ok(None)` is the typed miss: no lyrics for this track (non-200, or a
  /// 200 envelope without a usable `lyrics_url`, or an unparsable body).
  /// `Err` is reserved for transport/auth/offline failures — the lyrics
  /// orchestrator degrades those to a miss as well (spec §1.1); nothing in
  /// this path is ever a user-facing error.
  pub async fn get_lyrics_url(
    &self,
    track_id: u64,
    language: Option<&str>,
  ) -> Result<Option<QobuzLyricsUrls>> {
    let (status, body) = self.get_lyrics_url_raw(track_id, language).await?;
    if status != 200 {
      // Live-observed miss: HTTP 404 with body
      // {"status":"error","code":404,"message":"No lyrics found"}
      // (sample: qbz-nix-docs/qobuz-api/lyrics-miss-response.json).
      log::debug!("[API] get_lyrics_url({}) status={} -> no lyrics", track_id, status);
      return Ok(None);
    }
    match serde_json::from_str::<QobuzLyricsUrls>(&body) {
      Ok(urls) if urls.lyrics_url.is_some() => Ok(Some(urls)),
      Ok(_) => {
        log::debug!("[API] get_lyrics_url({}) 200 without lyrics_url -> no lyrics", track_id);
        Ok(None)
      }
      Err(e) => {
        log::warn!("[API] get_lyrics_url({}) unparsable 200 body ({}) -> no lyrics", track_id, e);
        Ok(None)
      }
    }
  }

  /// Steps 1 + 2: fetch the full lyrics document for a track.
  ///
  /// The CDN GET goes through [`Self::http`] (the offline choke point) so
  /// offline mode fails fast. The returned `lyrics_url` is self-authorizing
  /// — a pre-signed CloudFront URL (`Expires`/`Signature`/`Key-Pair-Id`);
  /// no `X-App-Id`/auth headers are sent (verified live 2026-06-10, HTTP
  /// 200 on a bare GET). `Ok(None)` = typed miss, same contract as
  /// [`Self::get_lyrics_url`]; a document without usable content counts as
  /// a miss.
  ///
  /// `language` (ISO 639-1, API v10) requests a translation; `None` fetches
  /// original-only (the default flow). LIVE WIRE (verified 2026-07-22):
  /// when a translation is requested AND available, `lyrics_url` points at
  /// the TRANSLATED document (`<id>-<lang>.json`), which carries ONLY
  /// `translation` and no `original` — so this fn then fetches the
  /// original-only document too and merges both into one bilingual
  /// document. When requested but NOT available the signed CDN URL 403s →
  /// typed miss (fail soft). When the document has an `original` but does
  /// not EMBED a `translation` block, the envelope's
  /// `translation_requested.lyrics_url` is fetched as a fallback and its
  /// content becomes the translation (fail soft: any gap in that fallback
  /// just leaves the document original-only).
  pub async fn get_lyrics(
    &self,
    track_id: u64,
    language: Option<&str>,
  ) -> Result<Option<QobuzLyricsDocument>> {
    let doc = match self.fetch_lyrics_doc(track_id, language).await? {
      Some(doc) => doc,
      None => return Ok(None),
    };

    if language.is_some() && doc.original.is_none() && doc.translation.is_some() {
      // Translation-only document (the common v10 live shape): fetch the
      // original document separately and merge into one bilingual doc.
      return match self.fetch_lyrics_doc(track_id, None).await? {
        Some(mut original_doc) if original_doc.original.is_some() => {
          merge_translation_into(&mut original_doc, doc);
          Ok(Some(original_doc))
        }
        _ => {
          log::debug!(
            "[API] get_lyrics({}) translation found but original doc missing -> no lyrics",
            track_id
          );
          Ok(None)
        }
      };
    }

    // Preserve the original-only miss semantics: a document with neither
    // `original` nor a mergeable `translation` is unusable.
    if doc.original.is_none() {
      log::debug!("[API] get_lyrics({}) document without original content -> no lyrics", track_id);
      return Ok(None);
    }
    Ok(Some(doc))
  }

  /// One lyrics round trip (signed lyricsUrl + bare CDN GET). Accepts any
  /// document carrying `original` OR `translation` content; the caller
  /// decides which shapes are usable (see [`Self::get_lyrics`]).
  pub(crate) async fn fetch_lyrics_doc(
    &self,
    track_id: u64,
    language: Option<&str>,
  ) -> Result<Option<QobuzLyricsDocument>> {
    let urls = match self.get_lyrics_url(track_id, language).await? {
      Some(urls) => urls,
      None => return Ok(None),
    };
    let lyrics_url = match urls.lyrics_url.clone() {
      Some(url) => url,
      None => return Ok(None),
    };

    let response = self.http()?.get(&lyrics_url).send().await?;
    let status = response.status();
    if !status.is_success() {
      log::debug!("[API] get_lyrics({}) CDN doc status={} -> no lyrics", track_id, status);
      return Ok(None);
    }
    let body = response.text().await?;
    match serde_json::from_str::<QobuzLyricsDocument>(&body) {
      Ok(mut doc) if doc.original.is_some() || doc.translation.is_some() => {
        if language.is_some() && doc.original.is_some() && doc.translation.is_none() {
          doc.translation = self.fetch_translation_fallback(track_id, &urls).await;
        }
        Ok(Some(doc))
      }
      Ok(_) => {
        log::debug!(
          "[API] get_lyrics({}) document without any lyrics content -> no lyrics",
          track_id
        );
        Ok(None)
      }
      Err(e) => {
        log::warn!("[API] get_lyrics({}) unparsable CDN doc ({}) -> no lyrics", track_id, e);
        Ok(None)
      }
    }
  }

  /// Fallback translation source (spec §B.3): when the main document did
  /// not embed a `translation` block, fetch the document the envelope's
  /// `translation_requested.lyrics_url` points at (same bare CDN GET) and
  /// take its content as the translation. Every failure mode degrades to
  /// `None` — translation is best-effort, never an error.
  pub(crate) async fn fetch_translation_fallback(
    &self,
    track_id: u64,
    urls: &QobuzLyricsUrls,
  ) -> Option<QobuzLyricsContent> {
    let translation_url =
      urls.translation_requested.as_ref().and_then(|t| t.lyrics_url.as_deref())?;
    let response = self.http().ok()?.get(translation_url).send().await.ok()?;
    if !response.status().is_success() {
      log::debug!(
        "[API] get_lyrics({}) translation doc status={} -> original only",
        track_id,
        response.status()
      );
      return None;
    }
    let body = response.text().await.ok()?;
    match serde_json::from_str::<QobuzLyricsDocument>(&body) {
      // The translation document carries its content under `original`
      // (same wrapper shape as the main document).
      Ok(doc) => doc.original,
      Err(e) => {
        log::debug!(
          "[API] get_lyrics({}) unparsable translation doc ({}) -> original only",
          track_id,
          e
        );
        None
      }
    }
  }

  /// Get artist by ID (basic info only - no albums, faster response)
  pub async fn get_artist_basic(&self, artist_id: u64) -> Result<Artist> {
    let url = endpoints::build_url(paths::ARTIST_GET);
    let locale = self.locale().await;
    let query = vec![
      ("artist_id", artist_id.to_string()),
      ("lang", locale),
      // No "extra" parameter = only basic info (id, name, image)
    ];

    let http_response = self
      .signed_get(
        &url,
        "artistget",
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?;
    log::debug!("[API] get_artist_basic({}) status={}", artist_id, http_response.status());
    let response: Value = http_response.json().await?;

    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get artist by ID
  pub async fn get_artist(&self, artist_id: u64, with_albums: bool) -> Result<Artist> {
    self.get_artist_with_pagination(artist_id, with_albums, None, None).await
  }

  /// Get artist detail by ID with albums, playlists, and appears-on tracks
  pub async fn get_artist_detail(
    &self,
    artist_id: u64,
    limit: Option<u32>,
    offset: Option<u32>,
  ) -> Result<Artist> {
    let url = endpoints::build_url(paths::ARTIST_GET);
    let locale = self.locale().await;
    let mut query = vec![
      ("artist_id", artist_id.to_string()),
      ("extra", "albums,tracks_appears_on,playlists".to_string()),
      ("lang", locale),
    ];
    if let Some(l) = limit {
      query.push(("limit", l.to_string()));
    }
    if let Some(o) = offset {
      query.push(("offset", o.to_string()));
    }

    let http_response = self
      .signed_get(
        &url,
        "artistget",
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?;
    log::debug!("[API] get_artist_detail({}) status={}", artist_id, http_response.status());
    let response: Value = http_response.json().await?;

    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get artist by ID with album pagination
  pub async fn get_artist_with_pagination(
    &self,
    artist_id: u64,
    with_albums: bool,
    limit: Option<u32>,
    offset: Option<u32>,
  ) -> Result<Artist> {
    self.get_artist_with_pagination_and_locale(artist_id, with_albums, limit, offset, None).await
  }

  /// Get artist by ID with album pagination and optional locale override
  /// Use locale_override to force a specific language (e.g., "en" for genre checking)
  pub async fn get_artist_with_pagination_and_locale(
    &self,
    artist_id: u64,
    with_albums: bool,
    limit: Option<u32>,
    offset: Option<u32>,
    locale_override: Option<&str>,
  ) -> Result<Artist> {
    let url = endpoints::build_url(paths::ARTIST_GET);
    let locale = match locale_override {
      Some(l) => l.to_string(),
      None => self.locale().await,
    };
    let mut query = vec![("artist_id", artist_id.to_string()), ("lang", locale)];
    if with_albums {
      query.push(("extra", "albums".to_string()));
    }
    if let Some(l) = limit {
      query.push(("limit", l.to_string()));
    }
    if let Some(o) = offset {
      query.push(("offset", o.to_string()));
    }

    let http_response = self
      .signed_get(
        &url,
        "artistget",
        &query.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
      )
      .await?;
    log::debug!(
      "[API] get_artist({}, albums={}) status={}",
      artist_id,
      with_albums,
      http_response.status()
    );
    let response: Value = http_response.json().await?;

    Ok(qobuz_models::artist_display::from_catalog_value(response)?)
  }

  /// Get playlist metadata + ordered track IDs (lightweight, no full Track objects).
  /// Uses `playlist/get?extra=track_ids` which returns the playlist with a flat
  /// array of track IDs instead of nested Track objects.
  pub async fn get_playlist_track_ids(&self, playlist_id: u64) -> Result<PlaylistWithTrackIds> {
    let url = endpoints::build_url(paths::PLAYLIST_GET);
    let http_response = self
      .signed_get(
        &url,
        "playlistget",
        &[("playlist_id", playlist_id.to_string()), ("extra", "track_ids".to_string())],
      )
      .await?;
    log::debug!("[API] get_playlist_track_ids({}) status={}", playlist_id, http_response.status());
    let response: Value = http_response.json().await?;
    let result: PlaylistWithTrackIds = qobuz_models::artist_display::from_catalog_value(response)?;
    log::info!(
      "[API] get_playlist_track_ids({}) — {} track IDs",
      playlist_id,
      result.track_ids.len()
    );
    Ok(result)
  }

  /// Fetch full Track objects for a batch of track IDs.
  /// Uses the `track/getList` endpoint, which caps at 50 IDs per call,
  /// so larger inputs are split into 50-ID windows and fetched serially.
  /// Input order is preserved in the returned vector.
  pub async fn get_tracks_batch(&self, track_ids: &[u64]) -> Result<Vec<Track>> {
    const MAX_PER_CALL: usize = 50;

    if track_ids.is_empty() {
      return Ok(Vec::new());
    }

    if track_ids.len() <= MAX_PER_CALL {
      return self.get_tracks_batch_chunk(track_ids).await;
    }

    log::debug!(
      "[API] get_tracks_batch chunking {} IDs into {}-windows",
      track_ids.len(),
      MAX_PER_CALL
    );
    let mut all = Vec::with_capacity(track_ids.len());
    for chunk in track_ids.chunks(MAX_PER_CALL) {
      let mut tracks = self.get_tracks_batch_chunk(chunk).await?;
      all.append(&mut tracks);
    }
    Ok(all)
  }

  /// Single `track/getList` POST. Caller is responsible for keeping
  /// `track_ids.len() <= 50` — `get_tracks_batch` handles that.
  pub(crate) async fn get_tracks_batch_chunk(&self, track_ids: &[u64]) -> Result<Vec<Track>> {
    let url = endpoints::build_url(paths::TRACK_GET_LIST);
    let headers = self.api_headers().await?;

    let body = serde_json::json!({ "tracks_id": track_ids });
    log::debug!("[API] get_tracks_batch POST ({} IDs)", track_ids.len());

    let http_response = self.http()?.post(&url).headers(headers).json(&body).send().await?;

    let status = http_response.status();
    log::debug!("[API] get_tracks_batch POST status={}", status);

    let value: Value = http_response.json().await?;

    // Response: { "tracks": { "total": N, "items": [...] } }
    let items = value.get("tracks").and_then(|t| t.get("items")).ok_or_else(|| {
      let preview =
        serde_json::to_string(&value).unwrap_or_default().chars().take(500).collect::<String>();
      ApiError::ApiResponse(format!("Missing tracks.items in getList response: {}", preview))
    })?;

    let tracks: Vec<Track> = qobuz_models::artist_display::from_catalog_value(items.clone())?;
    log::debug!("[API] get_tracks_batch returned {} tracks", tracks.len());
    Ok(tracks)
  }

  /// Get playlist by ID (paginates automatically to fetch all tracks)
  ///
  /// After the first page, remaining pages are fetched concurrently
  /// since we know the total track count from the first response.
  pub async fn get_playlist(&self, playlist_id: u64) -> Result<Playlist> {
    let url = endpoints::build_url(paths::PLAYLIST_GET);
    const PAGE_SIZE: u32 = 500;

    let start = std::time::Instant::now();

    // First page — gives us metadata + total track count
    let http_response = self
      .signed_get(
        &url,
        "playlistget",
        &[
          ("playlist_id", playlist_id.to_string()),
          ("limit", PAGE_SIZE.to_string()),
          ("offset", "0".to_string()),
          ("extra", "tracks".to_string()),
        ],
      )
      .await?;
    log::debug!("[API] get_playlist({}) status={}", playlist_id, http_response.status());
    let response: Value = http_response.json().await?;
    let mut playlist: Playlist = qobuz_models::artist_display::from_catalog_value(response)?;

    // Fetch remaining pages concurrently
    if let Some(ref mut container) = playlist.tracks {
      let total = container.total;
      let fetched = container.items.len() as u32;

      if fetched < total {
        // Build all remaining page offsets
        let offsets: Vec<u32> = (fetched..total).step_by(PAGE_SIZE as usize).collect();
        log::debug!(
          "[API] get_playlist({}) fetching {} remaining pages concurrently ({}/{})",
          playlist_id,
          offsets.len(),
          fetched,
          total
        );

        // Prepare headers and per-page signatures for concurrent requests
        let headers = self.api_headers().await?;

        // Offline gate checked ONCE for the whole page batch (the
        // first page above already passed through it) — not inside
        // the per-page loop.
        let gated_http = self.http()?;

        // Launch all page requests concurrently
        let futures: Vec<_> = offsets
          .iter()
          .map(|&offset| {
            let http = gated_http;
            let url = &url;
            let headers = headers.clone();
            let pid = playlist_id.to_string();
            let limit = PAGE_SIZE.to_string();
            let offset_str = offset.to_string();
            async move {
              let resp = http
                .get(url)
                .headers(headers)
                .query(&[
                  ("playlist_id", pid.as_str()),
                  ("limit", limit.as_str()),
                  ("offset", offset_str.as_str()),
                  ("extra", "tracks"),
                ])
                .send()
                .await?;
              let value: Value = resp.json().await?;
              let page: Playlist = qobuz_models::artist_display::from_catalog_value(value)?;
              Ok::<_, anyhow::Error>((offset, page))
            }
          })
          .collect();

        let results = futures_util::future::join_all(futures).await;

        // Collect results sorted by offset to maintain track order
        let mut pages: Vec<(u32, Playlist)> = Vec::new();
        for result in results {
          match result {
            Ok(page) => pages.push(page),
            Err(e) => {
              log::warn!("[API] get_playlist({}) page fetch failed: {}", playlist_id, e);
              // Continue with what we have
            }
          }
        }
        pages.sort_by_key(|(offset, _)| *offset);

        // Append tracks in order
        for (_, page_playlist) in pages {
          if let Some(page_tracks) = page_playlist.tracks {
            if !page_tracks.items.is_empty() {
              container.items.extend(page_tracks.items);
            }
          }
        }
      }
    }

    let elapsed = start.elapsed();
    log::debug!(
      "[API] get_playlist({}) complete: {} tracks in {:.2}s",
      playlist_id,
      playlist.tracks.as_ref().map(|t| t.items.len()).unwrap_or(0),
      elapsed.as_secs_f64()
    );

    Ok(playlist)
  }
}
