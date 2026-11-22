//! Playback reports match the desktop client's two POST encodings.
use crate::{
  client::QobuzClient,
  error::{ApiError, Result},
};
use serde_json::{json, Value};

fn request(
  http: &reqwest::Client,
  url: &str,
  start: bool,
  events: &[Value],
  software_version: &str,
) -> reqwest::RequestBuilder {
  if start {
    http
      .post(url)
      .form(&[("events", serde_json::to_string(events).expect("JSON values serialize"))])
  } else {
    http
      .post(url)
      .json(&json!({"events":events,"renderer_context":{"software_version":software_version}}))
  }
}

impl QobuzClient {
  pub async fn report_streaming_start(&self, events: &[Value]) -> Result<()> {
    self.report_streaming(true, events, "").await
  }
  pub async fn report_streaming_end(&self, events: &[Value], software_version: &str) -> Result<()> {
    self.report_streaming(false, events, software_version).await
  }
  async fn report_streaming(
    &self,
    start: bool,
    events: &[Value],
    software_version: &str,
  ) -> Result<()> {
    if events.is_empty() {
      return Ok(());
    }
    self.forbidden_guard()?;
    let endpoint =
      if start { "/track/reportStreamingStart" } else { "/track/reportStreamingEndJson" };
    let response = request(
      self.http()?,
      &crate::endpoints::build_url(endpoint),
      start,
      events,
      software_version,
    )
    .headers(self.authenticated_headers().await?)
    .timeout(std::time::Duration::from_secs(8))
    .send()
    .await?;
    let status = response.status();
    self.note_forbidden_status(status);
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
      return Err(ApiError::RateLimited(
        response
          .headers()
          .get("retry-after")
          .and_then(|h| h.to_str().ok())
          .and_then(|h| h.parse().ok())
          .unwrap_or(30),
      ));
    }
    if status.is_server_error() {
      return Err(ApiError::ServerError(status.as_u16()));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
      return Err(ApiError::AuthenticationError("Playback report needs sign-in".into()));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
      return Err(ApiError::Forbidden(String::new()));
    }
    if !status.is_success() {
      return Err(ApiError::ApiResponse(format!(
        "Playback report returned HTTP {}",
        status.as_u16()
      )));
    }
    let body: Value = response.json().await?;
    if body.get("status").and_then(Value::as_str) != Some("success") {
      return Err(ApiError::ApiResponse("Playback report was not accepted".into()));
    }
    Ok(())
  }
}

#[cfg(test)]
#[path = "checks/reporting_tests.rs"]
mod checks;
