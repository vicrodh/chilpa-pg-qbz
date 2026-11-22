use super::client::*;

impl QobuzClient {
  // === Authenticated endpoints ===

  /// Get stream URL for a track (requires auth + signature)
  /// Raw `/track/getFileUrl` (`intent=stream`): HTTP status and body,
  /// verbatim, no parsing. Probe support only — the typed reader is
  /// [`Self::get_stream_url`]. Same signing, same headers, same breaker.
  pub async fn get_stream_url_raw(&self, track_id: u64, quality: Quality) -> Result<(u16, String)> {
    self.forbidden_guard()?;
    let url = endpoints::build_url(paths::TRACK_GET_FILE_URL);
    let timestamp = get_timestamp();
    let secret = self.secret().await?;
    let signature = sign_get_file_url(track_id, quality.id(), timestamp, &secret);
    let response = self
      .http()?
      .get(&url)
      .headers(self.authenticated_headers().await?)
      .query(&[
        ("track_id", track_id.to_string()),
        ("format_id", quality.id().to_string()),
        ("intent", "stream".to_string()),
        ("request_ts", timestamp.to_string()),
        ("request_sig", signature),
      ])
      .send()
      .await?;
    let status = response.status();
    self.note_forbidden_status(status);
    let body = response.text().await?;
    Ok((status.as_u16(), body))
  }

  pub async fn get_stream_url(&self, track_id: u64, quality: Quality) -> Result<StreamUrl> {
    // Back off before the network if the 403 breaker is open (issue #637).
    self.forbidden_guard()?;
    log::info!("Getting stream URL for track {} with quality {:?}", track_id, quality);
    let url = endpoints::build_url(paths::TRACK_GET_FILE_URL);
    let timestamp = get_timestamp();
    log::debug!("Getting secret for signing...");
    let secret = self.secret().await?;
    log::debug!("Secret obtained, signing request...");
    let signature = sign_get_file_url(track_id, quality.id(), timestamp, &secret);

    log::debug!("Sending stream URL request...");
    let response = self
      .http()?
      .get(&url)
      .headers(self.authenticated_headers().await?)
      .query(&[
        ("track_id", track_id.to_string()),
        ("format_id", quality.id().to_string()),
        ("intent", "stream".to_string()),
        ("request_ts", timestamp.to_string()),
        ("request_sig", signature),
      ])
      .send()
      .await?;

    let status = response.status();
    log::info!("Stream URL response status: {}", status);
    // Feed the breaker: a 403 counts toward opening it; a 200 resets it.
    self.note_forbidden_status(status);
    match status {
      StatusCode::OK => {
        let json: Value = response.json().await?;
        log::debug!(
          "Stream URL response JSON keys: {:?}",
          json.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );

        // Check for restrictions
        let restrictions: Vec<StreamRestriction> = json
          .get("restrictions")
          .and_then(|v| serde_json::from_value(v.clone()).ok())
          .unwrap_or_default();

        // Validate that we got an actual URL (track may be unavailable)
        let url = json["url"].as_str().unwrap_or("").to_string();
        if url.is_empty() {
          // Log the restriction codes for debugging
          let restriction_codes: Vec<&str> = restrictions.iter().map(|r| r.code.as_str()).collect();
          log::warn!(
            "Stream URL missing for track {} - restrictions: {:?}",
            track_id,
            restriction_codes
          );
          return Err(ApiError::TrackUnavailable(track_id));
        }

        Ok(StreamUrl {
          blob: json.get("blob").and_then(Value::as_str).map(str::to_owned),
          duration: json.get("duration").and_then(Value::as_f64),
          url,
          format_id: json["format_id"].as_u64().unwrap_or(0) as u32,
          mime_type: json["mime_type"].as_str().unwrap_or("").to_string(),
          sampling_rate: json["sampling_rate"].as_f64().unwrap_or(0.0),
          bit_depth: json["bit_depth"].as_u64().map(|v| v as u32),
          track_id,
          restrictions,
          sample: json["sample"].as_bool().unwrap_or(false),
        })
      }
      StatusCode::BAD_REQUEST => Err(ApiError::InvalidAppSecret),
      StatusCode::UNAUTHORIZED => Err(ApiError::AuthenticationError(
        "stream URL 401 — user auth token invalid or expired".to_string(),
      )),
      StatusCode::FORBIDDEN => {
        let preview = body_preview(response).await;
        log::warn!("Stream URL 403 for track {}{}", track_id, preview);
        Err(ApiError::Forbidden(preview))
      }
      status => Err(ApiError::ApiResponse(format!("Unexpected status: {}", status))),
    }
  }

  /// Get stream URL with quality fallback
  pub async fn get_stream_url_with_fallback(
    &self,
    track_id: u64,
    preferred: Quality,
  ) -> Result<StreamUrl> {
    self.get_stream_url_with_acquisition(track_id, preferred).await.map(|(url, _)| url)
  }

  /// Also return the quality of the successful request. The response's
  /// format_id describes the available file, not whether fallback occurred.
  pub async fn get_stream_url_with_acquisition(
    &self,
    track_id: u64,
    preferred: Quality,
  ) -> Result<(StreamUrl, Quality)> {
    Self::resolve_stream_url_with_fallback(track_id, preferred, |quality| {
      self.get_stream_url(track_id, quality)
    })
    .await
  }

  pub(crate) async fn resolve_stream_url_with_fallback<F, Fut>(
    track_id: u64,
    preferred: Quality,
    mut fetch: F,
  ) -> Result<(StreamUrl, Quality)>
  where
    F: FnMut(Quality) -> Fut,
    Fut: std::future::Future<Output = Result<StreamUrl>>,
  {
    log::info!(
      "Getting stream URL with fallback for track {}, preferred quality: {:?}",
      track_id,
      preferred
    );
    let qualities = Quality::fallback_order();
    let start_idx = qualities.iter().position(|q| *q == preferred).unwrap_or(0);

    let mut track_unavailable = false;

    for quality in &qualities[start_idx..] {
      log::info!("Trying quality: {:?}", quality);
      match fetch(*quality).await {
        Ok(url) if !url.has_restrictions() => {
          log::info!("Got stream URL for requested quality format_id={}", quality.id());
          return Ok((url, *quality));
        }
        Ok(_) => {
          log::info!("Quality {:?} has restrictions, trying next", quality);
          continue;
        }
        Err(ApiError::InvalidAppSecret) => {
          log::error!("Invalid app secret");
          return Err(ApiError::InvalidAppSecret);
        }
        // A 403 (or an open breaker, or a 401) is NOT a per-quality
        // restriction — every quality would 403 the same way. Abort the
        // whole fallback loop immediately instead of firing 5 more
        // requests per track and feeding the storm (issue #637).
        Err(
          e @ (ApiError::Forbidden(_)
          | ApiError::ForbiddenCircuitOpen(_)
          | ApiError::AuthenticationError(_)),
        ) => {
          log::warn!("Stream URL aborting quality fallback: {}", e);
          return Err(e);
        }
        Err(ApiError::TrackUnavailable(_)) => {
          // Track is completely unavailable on Qobuz
          track_unavailable = true;
          continue;
        }
        Err(e) => {
          log::warn!("Quality {:?} failed: {}, trying next", quality, e);
          continue;
        }
      }
    }

    // If all quality levels reported track unavailable, return that specific error
    if track_unavailable {
      log::error!("Track {} is no longer available on Qobuz", track_id);
      return Err(ApiError::TrackUnavailable(track_id));
    }

    log::error!("No quality available for track {}", track_id);
    Err(ApiError::NoQualityAvailable)
  }
}
