//! Host-managed browser setup; provider credentials are held only in memory.
use qobuz_api::QobuzClient;
use qobuz_models::UserSession;
use serde_json::{json, Value};
use std::{
  path::Path,
  sync::OnceLock,
  time::{Duration, Instant},
};
mod transport;
mod view;
use tokio::sync::Mutex;
use transport::BrowserConnection;

#[derive(Default)]
struct State {
  connection: Option<BrowserConnection>,
  revision: u64,
  last_form: Value,
  notice: Option<String>,
  last_warm: Option<Instant>,
}
pub struct AccessController {
  state: Mutex<State>,
  network: reqwest::Client,
  managed: OnceLock<Value>,
  activation: Mutex<()>,
}
impl AccessController {
  pub fn new(_directory: &Path) -> Self {
    Self {
      managed: OnceLock::new(),
      activation: Mutex::new(()),
      state: Mutex::new(State::default()),
      network: reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(19))
        .build()
        .expect("loopback transport"),
    }
  }
  pub fn configure(&self, value: Value) {
    let _ = self.managed.set(value);
  }
  async fn accept(&self, api: &QobuzClient, connection: &BrowserConnection) -> Result<(), String> {
    let value = self.request(connection, "session", json!({})).await?;
    let session: UserSession =
      serde_json::from_value(value["session"].clone()).map_err(|_| "Invalid Firefox account")?;
    let app = value["app_id"].as_str().ok_or("Missing browser app ID")?;
    api
      .configure_browser(app.into(), session)
      .await
      .map_err(|_| "Firefox account validation failed".into())
  }
  async fn complete_session(
    &self,
    api: &QobuzClient,
    connection: &BrowserConnection,
    setup: &Value,
  ) -> Result<(), String> {
    let _activation = self.activation.lock().await;
    if !api.is_logged_in().await {
      self.accept(api, connection).await?;
    }
    if setup["phase"] == "verify" {
      if let Err(error) = self.request(connection, "setup", json!({"action":"verified"})).await {
        api.logout().await;
        return Err(error);
      }
    }
    self.state.lock().await.notice = None;
    Ok(())
  }
  /// The helper the host started, taken on first use: the settings form can
  /// open before the startup load has run.
  async fn connection(&self) -> Result<Option<BrowserConnection>, String> {
    let mut state = self.state.lock().await;
    if state.connection.is_none() {
      let value = self.managed.get().cloned().unwrap_or(Value::Null);
      if value["protocol"] == 1 {
        let connection: BrowserConnection =
          serde_json::from_value(value).map_err(|_| "Invalid desktop connection")?;
        connection.validate()?;
        state.connection = Some(connection);
      }
    }
    Ok(state.connection.clone())
  }
  pub async fn load(&self, api: &QobuzClient) -> Result<(), String> {
    if let Some(connection) = self.connection().await? {
      let setup = self.request(&connection, "setup", json!({})).await?;
      if matches!(setup["phase"].as_str(), Some("verify" | "ready")) {
        if let Err(error) = self.complete_session(api, &connection, &setup).await {
          self.state.lock().await.notice = Some(error);
        }
      }
    }
    Ok(())
  }
  pub async fn refresh(&self, api: &QobuzClient) -> Result<(), String> {
    let connection = self
      .state
      .lock()
      .await
      .connection
      .clone()
      .ok_or("Open the Qobuz plugin settings and sign in with Firefox")?;
    let setup = self.request(&connection, "setup", json!({})).await?;
    if !matches!(setup["phase"].as_str(), Some("verify" | "ready")) {
      return Err("Sign in with Firefox in the Qobuz plugin settings.".into());
    }
    self.complete_session(api, &connection, &setup).await
  }
  pub async fn warm(&self) {
    let connection = {
      let mut state = self.state.lock().await;
      if state.last_warm.is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
        return;
      }
      let Some(connection) = state.connection.clone() else { return };
      state.last_warm = Some(Instant::now());
      connection
    };
    let _ =
      tokio::time::timeout(Duration::from_secs(2), self.request(&connection, "warm", json!({})))
        .await;
  }
  pub async fn resolve(
    &self,
    track: u64,
    quality: u32,
    metadata: Option<Value>,
  ) -> Result<Value, String> {
    let connection = self
      .state
      .lock()
      .await
      .connection
      .clone()
      .ok_or("Open the Qobuz plugin settings and sign in with Firefox")?;
    self
      .request(
        &connection,
        "resolve",
        json!({"track":track.to_string(),"quality":quality,"metadata":metadata}),
      )
      .await
  }
  pub async fn stop(&self, resource: &str) {
    let connection = self.state.lock().await.connection.clone();
    if let Some(connection) = connection {
      let _ = tokio::time::timeout(
        Duration::from_secs(1),
        self.request(&connection, "stop", json!({"ticket":resource})),
      )
      .await;
    }
  }
  pub async fn inspect(&self, api: &QobuzClient) -> Value {
    json!({"signed_in":api.is_logged_in().await,"pending":false})
  }
  pub async fn sign_out(&self, api: &QobuzClient) -> Result<(), String> {
    api.logout().await;
    let connection = self.state.lock().await.connection.clone();
    if let Some(connection) = connection {
      self.request(&connection, "setup", json!({"action":"disconnect"})).await?;
    }
    Ok(())
  }
  pub async fn apply(&self, api: &QobuzClient, language: &str, payload: Value) -> Value {
    let outcome = self.apply_action(api, &payload).await;
    self.state.lock().await.notice = outcome.err();
    self.form(api, language).await
  }
  async fn apply_action(&self, api: &QobuzClient, payload: &Value) -> Result<(), String> {
    if payload["revision"].as_u64() != Some(self.state.lock().await.revision) {
      return Err("Reopen settings and try again.".into());
    }
    let connection = self
      .state
      .lock()
      .await
      .connection
      .clone()
      .ok_or("Restart Qobuz to connect its desktop helper")?;
    let action = payload["action"].as_str().unwrap_or("");
    match action {
      "verify" => {
        self.accept(api, &connection).await?;
        self.request(&connection, "setup", json!({"action":"verified"})).await?;
      }
      "sign_out" => {
        self.request(&connection, "setup", json!({"action":"disconnect"})).await?;
        api.logout().await;
      }
      "select" | "login" | "cancel" | "detect" | "change" => {
        if action != "cancel" {
          api.logout().await;
        }
        self
          .request(
            &connection,
            "setup",
            json!({"action":action,
          "mode":payload["values"]["mode"].as_str().unwrap_or("native"),
          "path":payload["values"]["firefox"].as_str().unwrap_or(""),
          "ffmpeg":payload["values"]["ffmpeg"].as_str().unwrap_or("")}),
          )
          .await?;
      }
      _ => return Err("Unknown browser setup action".into()),
    }
    Ok(())
  }
}
#[cfg(test)]
mod tests;
