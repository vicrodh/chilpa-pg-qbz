//! Durable playback ledger. Accepting an event never performs network I/O.
mod events;
mod model;
mod sender;

use crate::files;
use model::{Ledger, Ticket, CAPACITY};
use qobuz_models::StreamUrl;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub(crate) struct PlaybackJournal {
  file: PathBuf,
  ledger: Mutex<Ledger>,
  sender: Mutex<()>,
}

impl PlaybackJournal {
  pub fn new(directory: &Path) -> Self {
    Self {
      file: directory.join("reports.json"),
      ledger: Mutex::new(Ledger::default()),
      sender: Mutex::new(()),
    }
  }
  pub async fn recover(&self) -> Result<(), String> {
    let mut ledger = self.ledger.lock().await;
    match files::load_json(&self.file) {
      Ok(Some(saved)) => *ledger = saved,
      Ok(None) => (),
      Err(error) => {
        ledger.last_error = Some(error);
        ledger.unreadable = true;
      }
    }
    Ok(())
  }
  pub async fn issue_ticket(
    &self,
    account: u64,
    audio: &StreamUrl,
    context: Option<String>,
  ) -> String {
    let mut ledger = self.ledger.lock().await;
    if ledger.tickets.len() >= CAPACITY {
      if let Some(oldest) = ledger.tickets.keys().next().cloned() {
        ledger.tickets.remove(&oldest);
      }
    }
    let key = uuid::Uuid::new_v4().to_string();
    ledger.tickets.insert(
      key.clone(),
      Ticket {
        account,
        track: audio.track_id,
        format: audio.format_id,
        receipt: audio.blob.clone(),
        seconds: audio.duration,
        preview: audio.sample,
        resource: audio
          .url
          .rsplit('/')
          .next()
          .filter(|value| value.len() == 32)
          .map(str::to_string),
        context,
      },
    );
    if !ledger.unreadable {
      if let Err(error) = files::store_json(&self.file, &*ledger) {
        ledger.last_error = Some(error);
      }
    }
    key
  }
  pub async fn active_resource(&self, playback: &str) -> Option<String> {
    self.ledger.lock().await.playing.get(playback).and_then(|entry| entry.ticket.resource.clone())
  }
  pub async fn record(&self, account: u64, event: &Value) -> Result<(), String> {
    let mut committed = self.ledger.lock().await;
    if committed.unreadable {
      return Err("The saved report queue needs recovery".into());
    }
    let mut transaction = committed.clone();
    transaction.advance(account, event)?;
    files::store_json(&self.file, &transaction)?;
    *committed = transaction;
    Ok(())
  }
  pub async fn recent(&self, account: u64) -> Vec<Value> {
    self.ledger.lock().await.history.iter().filter(|row| row["user"] == account).cloned().collect()
  }
  pub async fn inspect(&self) -> Value {
    let ledger = self.ledger.lock().await;
    json!({"queued":ledger.outbox.len(),"active":ledger.playing.len(),"last_error":ledger.last_error})
  }
  pub async fn reset(&self) -> Result<(), String> {
    let _delivery = self.sender.lock().await;
    let mut ledger = self.ledger.lock().await;
    files::store_json(&self.file, &Ledger::default())?;
    *ledger = Ledger::default();
    Ok(())
  }
}

#[cfg(test)]
#[path = "../checks/history.rs"]
mod checks;
