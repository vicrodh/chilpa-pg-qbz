use super::content::MAX_DOCUMENT;
use crate::runtime::RpcFault;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::{
  path::Path,
  sync::{Arc, Mutex},
};

const MAX_ENTRIES: usize = 16;
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;

fn fault(_: impl std::fmt::Display) -> RpcFault {
  RpcFault::new(-32000, "Cannot read the magazine cache")
}

pub(super) struct AccountCache {
  connection: Mutex<Connection>,
  pub gate: tokio::sync::Mutex<()>,
}

impl AccountCache {
  pub fn open(directory: &Path, user: u64) -> Result<Arc<Self>, RpcFault> {
    std::fs::create_dir_all(directory).map_err(fault)?;
    let connection =
      Connection::open(directory.join(format!("magazine-{user}.sqlite3"))).map_err(fault)?;
    connection.busy_timeout(std::time::Duration::from_secs(2)).map_err(fault)?;
    connection.set_prepared_statement_cache_capacity(4);
    connection.execute_batch("PRAGMA cache_size=-512; PRAGMA mmap_size=0;
      PRAGMA temp_store=FILE; PRAGMA journal_mode=DELETE;
      CREATE TABLE IF NOT EXISTS profile(id INTEGER PRIMARY KEY CHECK(id=1),store TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS pages(key TEXT PRIMARY KEY,payload TEXT NOT NULL,updated_at INTEGER NOT NULL);").map_err(fault)?;
    let page_size: i64 =
      connection.pragma_query_value(None, "page_size", |row| row.get(0)).map_err(fault)?;
    connection
      .pragma_update(None, "max_page_count", MAX_FILE_BYTES as i64 / page_size)
      .map_err(fault)?;
    Ok(Arc::new(Self { connection: Mutex::new(connection), gate: tokio::sync::Mutex::new(()) }))
  }

  pub fn store(&self) -> Option<String> {
    self
      .connection
      .lock()
      .ok()?
      .query_row("SELECT store FROM profile WHERE id=1", [], |row| row.get::<_, String>(0))
      .optional()
      .ok()?
      .filter(|store| qobuz_api::editorial::valid_store(store))
  }

  pub fn set_store(&self, store: &str) -> Result<(), RpcFault> {
    if !qobuz_api::editorial::valid_store(store) {
      return Err(RpcFault::invalid("magazine store"));
    }
    let mut connection = self.connection.lock().map_err(fault)?;
    let tx = connection.transaction().map_err(fault)?;
    let previous = tx
      .query_row("SELECT store FROM profile WHERE id=1", [], |row| row.get::<_, String>(0))
      .optional()
      .map_err(fault)?;
    if previous.as_deref() != Some(store) {
      tx.execute("DELETE FROM pages", []).map_err(fault)?;
    }
    tx.execute("INSERT INTO profile(id,store) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET store=excluded.store",[store]).map_err(fault)?;
    tx.commit().map_err(fault)
  }

  pub fn get(&self, key: &str) -> Result<Option<(Value, i64)>, RpcFault> {
    let connection = self.connection.lock().map_err(fault)?;
    let row: Option<(String, i64)> = connection
      .query_row("SELECT payload,updated_at FROM pages WHERE key=?1", [key], |row| {
        Ok((row.get(0)?, row.get(1)?))
      })
      .optional()
      .map_err(fault)?;
    row
      .map(|(payload, at)| {
        if payload.len() > MAX_DOCUMENT {
          return Err(RpcFault::new(-32000, "The cached magazine page is too large"));
        }
        serde_json::from_str(&payload).map(|page| (page, at)).map_err(fault)
      })
      .transpose()
  }

  pub fn put(&self, key: &str, payload: &Value, now: i64) -> Result<(), RpcFault> {
    let payload = serde_json::to_string(payload).map_err(fault)?;
    if payload.len() > MAX_DOCUMENT || key.len() > 4096 {
      return Err(RpcFault::new(-32000, "The magazine page exceeds its cache limit"));
    }
    let mut connection = self.connection.lock().map_err(fault)?;
    let tx = connection.transaction().map_err(fault)?;
    tx.execute(
      "INSERT INTO pages(key,payload,updated_at) VALUES(?1,?2,?3)
      ON CONFLICT(key) DO UPDATE SET payload=excluded.payload,updated_at=excluded.updated_at",
      params![key, payload, now],
    )
    .map_err(fault)?;
    tx.execute(
      "DELETE FROM pages WHERE key IN
      (SELECT key FROM pages ORDER BY updated_at DESC,key LIMIT -1 OFFSET ?1)",
      [MAX_ENTRIES as i64],
    )
    .map_err(fault)?;
    tx.commit().map_err(fault)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn pages_survive_restart_but_never_cross_account_or_store() {
    let directory = tempfile::tempdir().unwrap();
    {
      let cache = AccountCache::open(directory.path(), 1).unwrap();
      cache.set_store("us-en").unwrap();
      cache.put("article", &json!({"title":"Saved"}), 1).unwrap();
    }
    let cache = AccountCache::open(directory.path(), 1).unwrap();
    assert_eq!(cache.store().as_deref(), Some("us-en"));
    assert_eq!(cache.get("article").unwrap().unwrap().0["title"], "Saved");
    assert!(AccountCache::open(directory.path(), 2).unwrap().get("article").unwrap().is_none());
    cache.set_store("es-es").unwrap();
    assert!(cache.get("article").unwrap().is_none());
  }

  #[test]
  fn old_pages_are_evicted_and_oversized_documents_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let cache = AccountCache::open(directory.path(), 1).unwrap();
    for n in 0..20 {
      cache.put(&format!("story:{n}"), &json!({"id":n}), n).unwrap();
    }
    assert!(cache.get("story:0").unwrap().is_none());
    assert!(cache.get("story:19").unwrap().is_some());
    assert!(cache.put("huge", &json!({"text":"x".repeat(MAX_DOCUMENT)}), 21).is_err());
    assert_eq!(
      cache
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM pages", [], |row| row.get::<_, i64>(0))
        .unwrap(),
      MAX_ENTRIES as i64
    );
  }
}
