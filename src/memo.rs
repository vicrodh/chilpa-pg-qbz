use serde_json::Value;
use std::{
  collections::HashMap,
  sync::{Arc, Mutex, Weak},
  time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

struct Record {
  created: Instant,
  lifetime: Duration,
  payload: Value,
}
impl Record {
  fn alive(&self) -> bool {
    self.created.elapsed() < self.lifetime
  }
}

#[derive(Default)]
struct Entries {
  records: HashMap<String, Record>,
  reservations: HashMap<String, Weak<AsyncMutex<()>>>,
}

#[derive(Default)]
pub(crate) struct ReplyMemo(Mutex<Entries>);

impl ReplyMemo {
  pub fn get(&self, key: &str) -> Option<Value> {
    let entries = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
    entries.records.get(key).filter(|record| record.alive()).map(|record| record.payload.clone())
  }

  pub fn put(&self, key: &str, lifetime: Duration, payload: &Value) {
    let mut entries = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
    entries.records.retain(|_, record| record.alive());
    if entries.records.len() >= 512 {
      if let Some(oldest) =
        entries.records.iter().min_by_key(|(_, record)| record.created).map(|(key, _)| key.clone())
      {
        entries.records.remove(&oldest);
      }
    }
    entries.records.insert(
      key.to_owned(),
      Record { created: Instant::now(), lifetime, payload: payload.clone() },
    );
  }

  pub fn clear(&self) {
    self.0.lock().unwrap_or_else(|poison| poison.into_inner()).records.clear();
  }

  pub fn remove_prefix(&self, prefix: &str) {
    self
      .0
      .lock()
      .unwrap_or_else(|poison| poison.into_inner())
      .records
      .retain(|key, _| !key.starts_with(prefix));
  }

  pub async fn reserve(&self, key: &str) -> OwnedMutexGuard<()> {
    let reservation = {
      let mut entries = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
      entries.reservations.retain(|_, entry| entry.strong_count() > 0);
      match entries.reservations.get(key).and_then(Weak::upgrade) {
        Some(existing) => existing,
        None => {
          let created = Arc::new(AsyncMutex::new(()));
          entries.reservations.insert(key.to_owned(), Arc::downgrade(&created));
          created
        }
      }
    };
    reservation.lock_owned().await
  }
}

#[cfg(test)]
#[path = "checks/memo.rs"]
mod checks;
