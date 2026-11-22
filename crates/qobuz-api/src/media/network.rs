use crate::retry::{classify_reqwest, classify_status, retry_transient, FetchError};
use std::time::Duration;

#[derive(Clone)]
pub struct SegmentSource {
  http: reqwest::Client,
  pub deadline: Duration,
  pub attempts: u32,
}

impl SegmentSource {
  pub fn new() -> Result<Self, String> {
    let http = reqwest::Client::builder()
      .connect_timeout(Duration::from_secs(10))
      .build()
      .map_err(|error| format!("CMAF client error: {error}"))?;
    Ok(Self { http, deadline: Duration::from_secs(15), attempts: 3 })
  }

  pub async fn read(&self, address: &str) -> Result<Vec<u8>, String> {
    retry_transient(self.attempts, "media segment", FetchError::is_transient, |_| async {
      let response = self
        .http
        .get(address)
        .header("User-Agent", "Mozilla/5.0")
        .timeout(self.deadline)
        .send()
        .await
        .map_err(|error| classify_reqwest(&error, "fetch"))?;
      if !response.status().is_success() {
        return Err(classify_status(response.status(), "fetch"));
      }
      response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| classify_reqwest(&error, "read"))
    })
    .await
    .map_err(|error| error.to_string())
  }
}
