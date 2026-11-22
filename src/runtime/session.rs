use super::RpcFault;
use crate::{access::AccessController, history::PlaybackJournal};
use qobuz_api::QobuzClient;
use std::{
  path::PathBuf,
  sync::{atomic::AtomicBool, Mutex},
};
use tokio::sync::OnceCell;

pub(crate) struct SessionContext {
  pub directory: PathBuf,
  pub cache: PathBuf,
  pub network: AtomicBool,
  pub api: QobuzClient,
  pub access: AccessController,
  pub journal: PlaybackJournal,
  loaded: OnceCell<()>,
  locale: Mutex<String>,
}

impl SessionContext {
  pub fn new(directory: PathBuf, cache: PathBuf) -> Self {
    Self {
      api: QobuzClient::new().expect("catalog transport"),
      access: AccessController::new(&directory),
      journal: PlaybackJournal::new(&directory),
      directory,
      cache,
      network: AtomicBool::new(true),
      loaded: OnceCell::new(),
      locale: Mutex::new("en".into()),
    }
  }

  pub async fn load(&self) -> Result<(), RpcFault> {
    if !self.network.load(std::sync::atomic::Ordering::Relaxed) {
      return Ok(());
    }
    self
      .loaded
      .get_or_try_init(|| async {
        self.access.load(&self.api).await.map_err(|error| RpcFault::new(-32000, error))?;
        self.journal.recover().await.map_err(|error| RpcFault::new(-32000, error))?;
        if let Some(language) = self.language() {
          self.api.set_locale(language).await;
        }
        Ok::<(), RpcFault>(())
      })
      .await
      .copied()
  }

  pub async fn require_identity(&self) -> Result<(), RpcFault> {
    if !self.network.load(std::sync::atomic::Ordering::Relaxed) {
      return Err(RpcFault::new(-32000, "Qobuz needs network permission"));
    }
    self.load().await?;
    if !self.api.is_logged_in().await {
      self.access.refresh(&self.api).await.map_err(|error| RpcFault::new(-32001, error))?;
    }
    if self.api.is_logged_in().await {
      Ok(())
    } else {
      Err(RpcFault::new(-32001, "Sign in with Firefox in the Qobuz plugin settings."))
    }
  }

  pub fn set_locale(&self, locale: &str) {
    *self.locale.lock().unwrap_or_else(|poison| poison.into_inner()) = locale.to_owned();
  }

  pub fn language(&self) -> Option<String> {
    self
      .locale
      .lock()
      .unwrap_or_else(|poison| poison.into_inner())
      .split(['-', '_'])
      .next()
      .filter(|language| language.len() == 2)
      .map(str::to_lowercase)
  }
}
