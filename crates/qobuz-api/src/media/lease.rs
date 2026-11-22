use crate::{
  client::{body_preview, QobuzClient},
  endpoints::{self, paths},
  error::{ApiError, Result},
  signing,
};
use qobuz_models::{Quality, SessionStartResponse, TrackFileUrl};
use reqwest::StatusCode;

#[derive(Clone)]
pub(crate) struct MediaLease {
  pub id: String,
  pub descriptor: String,
  deadline: u64,
}

pub(crate) struct Location {
  pub file: TrackFileUrl,
  pub lease: MediaLease,
}

impl QobuzClient {
  async fn acquire_media_lease(&self) -> Result<MediaLease> {
    // One owner renews a lease. Key derivation travels with that exact lease.
    let mut slot = self.media_lease.lock().await;
    if let Some(lease) =
      slot.as_ref().filter(|lease| lease.deadline > signing::get_timestamp() + 60)
    {
      return Ok(lease.clone());
    }
    self.forbidden_guard()?;
    let stamp = signing::get_timestamp();
    let response = self
      .http()?
      .post(endpoints::build_url(paths::SESSION_START))
      .headers(self.authenticated_headers().await?)
      .form(&[
        ("profile", "qbz-1".to_owned()),
        ("request_ts", stamp.to_string()),
        ("request_sig", signing::sign_session_start(stamp)),
      ])
      .send()
      .await?;
    self.note_forbidden_status(response.status());
    match response.status() {
      status if status.is_success() => {
        let body: SessionStartResponse = response.json().await?;
        let lease = MediaLease {
          id: body.session_id,
          descriptor: body.infos.unwrap_or_default(),
          deadline: body.expires_at,
        };
        *slot = Some(lease.clone());
        Ok(lease)
      }
      StatusCode::FORBIDDEN => Err(ApiError::Forbidden(body_preview(response).await)),
      status => Err(ApiError::ApiResponse(format!("session/start failed with status {status}"))),
    }
  }

  pub(crate) async fn locate_media(&self, track: u64, quality: Quality) -> Result<Location> {
    self.forbidden_guard()?;
    crate::retry::retry_transient(
      crate::retry::DEFAULT_MAX_ATTEMPTS,
      "media location",
      ApiError::is_transient,
      |_| async {
        let lease = self.acquire_media_lease().await?;
        let stamp = signing::get_timestamp();
        let mut headers = self.authenticated_headers().await?;
        headers.insert(
          "X-Session-Id",
          lease
            .id
            .parse()
            .map_err(|_| ApiError::ApiResponse("Invalid session ID format".into()))?,
        );
        let response = self
          .http()?
          .get(endpoints::build_url(paths::FILE_URL))
          .headers(headers)
          .query(&[
            ("track_id", track.to_string()),
            ("format_id", quality.id().to_string()),
            ("intent", "stream".to_owned()),
            ("request_ts", stamp.to_string()),
            ("request_sig", signing::sign_file_url(track, quality.id(), stamp)),
          ])
          .send()
          .await?;
        self.note_forbidden_status(response.status());
        let status = response.status();
        if status.is_success() {
          return Ok(Location { file: response.json().await?, lease });
        }
        Err(match status {
          StatusCode::FORBIDDEN => ApiError::Forbidden(body_preview(response).await),
          StatusCode::NOT_FOUND => ApiError::TrackUnavailable(track),
          StatusCode::TOO_MANY_REQUESTS => ApiError::RateLimited(
            response
              .headers()
              .get(reqwest::header::RETRY_AFTER)
              .and_then(|value| value.to_str().ok())
              .and_then(|value| value.trim().parse().ok())
              .unwrap_or(2),
          ),
          status if status.is_server_error() => ApiError::ServerError(status.as_u16()),
          status => ApiError::ApiResponse(format!("file/url failed with status {status}")),
        })
      },
    )
    .await
  }
}
