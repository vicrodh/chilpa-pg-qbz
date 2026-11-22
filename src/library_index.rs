use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
  path::Path,
  sync::{
    atomic::{AtomicI64, Ordering},
    Mutex, MutexGuard,
  },
};

pub(crate) mod artwork;
pub(crate) mod awards;
mod derived;
#[cfg(test)]
mod derived_tests;
mod equality;
mod local;
#[cfg(test)]
mod local_tests;
mod member;
mod query;
#[cfg(test)]
mod revision_tests;
mod revisions;
mod schema;
mod seed;
#[cfg(test)]
mod tests;
mod writer;

pub(crate) type IndexResult<T> = Result<T, String>;
pub(crate) type Revisions = std::collections::BTreeMap<String, Option<i64>>;
pub(crate) type LocalVersions = std::collections::BTreeMap<String, LocalVersion>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LocalVersion {
  pub version: u64,
  pub applied: u64,
}
pub(crate) const MAX_CHUNK: usize = 500;
pub(crate) const MAX_PAYLOAD: usize = 64 * 1024;
pub(crate) const MAX_PAGE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Membership {
  Favorites,
  Purchases,
  Followed,
  Subscribed,
}

impl Membership {
  fn name(self) -> &'static str {
    match self {
      Self::Favorites => "favorites",
      Self::Purchases => "purchases",
      Self::Followed => "followed",
      Self::Subscribed => "subscribed",
    }
  }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Generation {
  account: u64,
  id: i64,
}

pub(crate) struct IndexedItem<'a> {
  pub item: &'a Value,
  pub added_at: i64,
  pub explicit: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IngestStats {
  pub new_entities: u64,
  pub new_memberships: u64,
}

#[derive(Debug)]
pub(crate) struct SeedProgress {
  pub stats: IngestStats,
  pub next: Option<String>,
  pub done: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Order {
  #[default]
  Added,
  Title,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Query {
  pub kinds: Vec<String>,
  pub genres: Vec<String>,
  pub owners: Vec<String>,
  pub exclude_owners: Vec<String>,
  pub membership_groups: Vec<Vec<Membership>>,
  pub hires: Option<bool>,
  pub text: String,
  pub explicit_only: bool,
  pub exclude_purchases: bool,
  pub order: Order,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct IndexStatus {
  pub account: u64,
  pub generation: i64,
  pub revision: i64,
  pub checked_at: i64,
  pub items: u64,
}

#[derive(Debug)]
pub(crate) struct Page {
  pub items: Vec<Value>,
  pub next: Option<String>,
  pub has_more: bool,
  pub status: IndexStatus,
}

#[derive(Debug)]
pub(crate) struct PruneProgress {
  pub removed: usize,
  pub pending: bool,
}

pub(crate) struct LibraryIndex {
  account: u64,
  connection: Mutex<Connection>,
  reader: Mutex<Connection>,
  failed_generation: AtomicI64,
  artwork_guard: tokio::sync::Mutex<()>,
  awards_guard: tokio::sync::Mutex<()>,
}

impl LibraryIndex {
  pub fn exists(directory: &Path, account: u64) -> IndexResult<bool> {
    directory
      .join(format!("library-{account}.sqlite3"))
      .try_exists()
      .map_err(|_| "The library index path could not be checked".into())
  }

  pub fn open(directory: &Path, account: u64) -> IndexResult<Self> {
    std::fs::create_dir_all(directory)
      .map_err(|_| "The library index directory could not be created")?;
    let path = directory.join(format!("library-{account}.sqlite3"));
    let mut connection = Connection::open(&path).map_err(schema::error)?;
    schema::initialize(&mut connection, account)?;
    let reader = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
      .map_err(schema::error)?;
    schema::initialize_reader(&reader)?;
    Ok(Self {
      account,
      connection: Mutex::new(connection),
      reader: Mutex::new(reader),
      failed_generation: AtomicI64::new(0),
      artwork_guard: tokio::sync::Mutex::new(()),
      awards_guard: tokio::sync::Mutex::new(()),
    })
  }

  fn lock(&self) -> MutexGuard<'_, Connection> {
    self.connection.lock().unwrap_or_else(|poison| poison.into_inner())
  }

  pub fn status(&self) -> IndexResult<IndexStatus> {
    schema::status(&self.read(), self.account)
  }

  fn read(&self) -> MutexGuard<'_, Connection> {
    self.reader.lock().unwrap_or_else(|poison| poison.into_inner())
  }

  // Cleanup belongs to the sync orchestrator, never to a visible-page query.
  pub fn prune_chunk(&self, limit: usize) -> IndexResult<PruneProgress> {
    let limit = limit.clamp(1, MAX_CHUNK) as i64;
    let mut connection = self.lock();
    let transaction = connection.transaction().map_err(schema::error)?;
    let obsolete: Option<i64> = transaction
      .query_row(
        "SELECT id FROM generations WHERE state!='building'
        AND id!=(SELECT current_generation FROM metadata WHERE id=1) ORDER BY id LIMIT 1",
        [],
        |row| row.get(0),
      )
      .optional()
      .map_err(schema::error)?;
    let Some(obsolete) = obsolete else {
      return Ok(PruneProgress { removed: 0, pending: false });
    };
    let removed = transaction
      .execute(
        "DELETE FROM entities WHERE generation=?1 AND (kind,reference) IN (
        SELECT kind,reference FROM entities WHERE generation=?1 ORDER BY kind,reference LIMIT ?2)",
        rusqlite::params![obsolete, limit],
      )
      .map_err(schema::error)?;
    transaction
      .execute(
        "DELETE FROM generations WHERE id=?1
        AND NOT EXISTS(SELECT 1 FROM entities e WHERE e.generation=?1)",
        [obsolete],
      )
      .map_err(schema::error)?;
    let pending = transaction
      .query_row(
        "SELECT EXISTS(SELECT 1 FROM generations WHERE state!='building'
        AND id!=(SELECT current_generation FROM metadata WHERE id=1))",
        [],
        |row| row.get(0),
      )
      .map_err(schema::error)?;
    transaction.commit().map_err(schema::error)?;
    Ok(PruneProgress { removed, pending })
  }

  fn building(connection: &Connection, account: u64, generation: Generation) -> IndexResult<()> {
    if generation.account != account {
      return Err("The generation belongs to another account".into());
    }
    let state: Option<String> = connection
      .query_row("SELECT state FROM generations WHERE id=?1", [generation.id], |row| row.get(0))
      .optional()
      .map_err(schema::error)?;
    match state.as_deref() {
      Some("building") => Ok(()),
      Some("failed") => Err("The library sync failed and cannot be published".into()),
      _ => Err("The library generation is no longer writable".into()),
    }
  }

  fn writable(&self, connection: &Connection, generation: Generation) -> IndexResult<()> {
    if self.failed_generation.load(Ordering::Relaxed) == generation.id {
      return Err("The library sync failed and cannot be published".into());
    }
    Self::building(connection, self.account, generation)
  }
}
