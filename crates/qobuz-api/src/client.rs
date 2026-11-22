//! Shared transport and account state. Authentication is supplied by the plugin.

pub(crate) use crate::endpoints::{self, paths};
pub(crate) use crate::error::{ApiError, Result};
use crate::forbidden_breaker::ForbiddenBreaker;
pub(crate) use crate::lyrics::{
  merge_translation_into, QobuzLyricsContent, QobuzLyricsDocument, QobuzLyricsUrls,
};
pub(crate) use crate::signing::{get_timestamp, sign_get_favorites, sign_get_file_url};
pub(crate) use qobuz_models::*;
pub(crate) use reqwest::{Client, StatusCode};
pub(crate) use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use zeroize::Zeroizing;

pub(crate) async fn body_preview(response: reqwest::Response) -> String {
  match response.text().await {
    Ok(body) => redacted_body_diagnostic(Zeroizing::new(body).as_str()).into(),
    Err(_) => String::new(),
  }
}
pub(crate) fn redacted_body_diagnostic(body: &str) -> &'static str {
  if body.trim().is_empty() {
    " : <empty body>"
  } else {
    " : <response body omitted>"
  }
}

struct AppCredentials {
  app_id: String,
  secret: Zeroizing<String>,
}

#[derive(Clone)]
pub struct QobuzClient {
  http: Client,
  credentials: Arc<RwLock<Option<AppCredentials>>>,
  session: Arc<RwLock<Option<UserSession>>>,
  locale: Arc<RwLock<String>>,
  pub(crate) media_lease: Arc<tokio::sync::Mutex<Option<crate::media::lease::MediaLease>>>,
  forbidden_breaker: Arc<ForbiddenBreaker>,
}

impl QobuzClient {
  pub fn new() -> Result<Self> {
    Ok(Self {
      http: Client::builder()
        .user_agent("Qobuz-Source/1.0")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?,
      credentials: Arc::new(RwLock::new(None)),
      session: Arc::new(RwLock::new(None)),
      locale: Arc::new(RwLock::new("en".into())),
      media_lease: Arc::new(tokio::sync::Mutex::new(None)),
      forbidden_breaker: Arc::new(ForbiddenBreaker::new()),
    })
  }

  /// Supply app credentials without discovery, extraction or network probes.
  pub async fn configure(&self, app_id: String, secret: String) -> Result<()> {
    if app_id.is_empty() || reqwest::header::HeaderValue::from_str(&app_id).is_err() {
      return Err(ApiError::InvalidAppId);
    }
    if secret.is_empty() {
      return Err(ApiError::InvalidAppSecret);
    }
    *self.credentials.write().await =
      Some(AppCredentials { app_id, secret: Zeroizing::new(secret) });
    self.logout().await;
    Ok(())
  }

  /// The official browser supplies the account session; catalogue reads need no app secret.
  pub async fn configure_browser(&self, app_id: String, session: UserSession) -> Result<()> {
    if app_id.is_empty() || session.user_id == 0 || session.user_auth_token.is_empty() {
      return Err(ApiError::AuthenticationError("Invalid browser session".into()));
    }
    reqwest::header::HeaderValue::from_str(&app_id).map_err(|_| ApiError::InvalidAppId)?;
    reqwest::header::HeaderValue::from_str(&session.user_auth_token)
      .map_err(|_| ApiError::AuthenticationError("Invalid browser session".into()))?;
    *self.credentials.write().await =
      Some(AppCredentials { app_id, secret: Zeroizing::new(String::new()) });
    self.set_session(session).await;
    Ok(())
  }

  pub async fn app_id(&self) -> Result<String> {
    self.credentials.read().await.as_ref().map(|c| c.app_id.clone()).ok_or(ApiError::InvalidAppId)
  }
  pub(crate) async fn secret(&self) -> Result<String> {
    self
      .credentials
      .read()
      .await
      .as_ref()
      .map(|c| c.secret.to_string())
      .ok_or(ApiError::InvalidAppSecret)
  }
  pub fn get_http(&self) -> &Client {
    &self.http
  }
  pub(crate) fn http(&self) -> Result<&Client> {
    if crate::offline_gate::is_offline() {
      return Err(ApiError::OfflineMode);
    }
    Ok(&self.http)
  }
  pub async fn set_locale(&self, locale: String) {
    *self.locale.write().await = locale;
  }
  pub async fn get_locale(&self) -> String {
    self.locale.read().await.clone()
  }
  pub(crate) async fn locale(&self) -> String {
    self.get_locale().await
  }
  pub async fn is_logged_in(&self) -> bool {
    self.session.read().await.is_some()
  }
  pub async fn logout(&self) {
    *self.session.write().await = None;
    *self.media_lease.lock().await = None;
  }
  pub async fn set_session(&self, session: UserSession) {
    *self.session.write().await = Some(session);
  }
  pub async fn session(&self) -> Option<UserSession> {
    self.session.read().await.clone()
  }
  pub async fn get_user_info(&self) -> Option<(String, String, Option<String>)> {
    self.session.read().await.as_ref().map(|s| {
      (s.display_name.clone(), s.subscription_label.clone(), s.subscription_valid_until.clone())
    })
  }
  pub async fn session_language_code(&self) -> Option<String> {
    self.session.read().await.as_ref().and_then(|s| s.language_code.clone())
  }
  pub async fn auth_token(&self) -> Result<String> {
    self
      .session
      .read()
      .await
      .as_ref()
      .map(|s| s.user_auth_token.clone())
      .ok_or_else(|| ApiError::AuthenticationError("Sign in first".into()))
  }
  pub(crate) fn forbidden_guard(&self) -> Result<()> {
    if let Some(left) = self.forbidden_breaker.blocked_for() {
      return Err(ApiError::ForbiddenCircuitOpen(left.as_secs()));
    }
    Ok(())
  }
  pub(crate) fn note_forbidden_status(&self, status: StatusCode) {
    if status == StatusCode::FORBIDDEN {
      self.forbidden_breaker.record_forbidden();
    } else if status.is_success() {
      self.forbidden_breaker.record_success();
    }
  }
}
