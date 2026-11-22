//! Private loopback endpoint supplied by the host, never by a pasted code.
use super::*;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub(super) struct BrowserConnection {
  pub port: u16,
  pub key: String,
}
impl BrowserConnection {
  pub fn validate(&self) -> Result<(), String> {
    if self.port <= 1024
      || !(32..=128).contains(&self.key.len())
      || !self.key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
      return Err("Invalid desktop browser connection".into());
    }
    Ok(())
  }
}

impl AccessController {
  pub(super) async fn request(
    &self,
    connection: &BrowserConnection,
    path: &str,
    body: Value,
  ) -> Result<Value, String> {
    let mut response = self
      .network
      .post(format!("http://127.0.0.1:{}/{}", connection.port, path))
      .timeout(Duration::from_secs(if path == "resolve" { 19 } else { 10 }))
      .bearer_auth(&connection.key)
      .json(&body)
      .send()
      .await
      .map_err(|_| "Restart Qobuz from the plugin manager and try again.")?;
    let status = response.status();
    if response.content_length().is_some_and(|n| n > 2 * 1024 * 1024) {
      return Err("Browser reply exceeded its limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "Browser reply interrupted")? {
      if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
        return Err("Browser reply exceeded its limit".into());
      }
      bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid browser reply")?;
    if !status.is_success() {
      if value["code"] == "session_expired" {
        return Err("The Firefox session expired. Sign in again in the Qobuz settings.".into());
      }
      return Err(
        match status.as_u16() {
          401 | 403 => "Reconnect Firefox from the Qobuz settings.",
          409 => "Finish signing in with Firefox. The account check completes automatically.",
          429 => "The browser is preparing other tracks. Try again shortly.",
          _ => "Firefox could not complete the request. Check the Qobuz settings.",
        }
        .into(),
      );
    }
    Ok(value)
  }
}
