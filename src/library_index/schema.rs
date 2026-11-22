use super::*;
use std::time::Duration;

pub(super) fn error(error: rusqlite::Error) -> String {
  format!("Library index: {error}")
}

pub(super) fn initialize(connection: &mut Connection, account: u64) -> IndexResult<()> {
  connection.busy_timeout(Duration::from_secs(5)).map_err(error)?;
  connection.set_prepared_statement_cache_capacity(8);
  connection
    .execute_batch(
      "PRAGMA page_size=4096;
    PRAGMA cache_size=-4096;
    PRAGMA mmap_size=0;
    PRAGMA temp_store=FILE;
    PRAGMA foreign_keys=ON;
    PRAGMA journal_mode=WAL;
    PRAGMA synchronous=FULL;
    PRAGMA wal_autocheckpoint=256;
    PRAGMA journal_size_limit=8388608;",
    )
    .map_err(error)?;
  let page_size: i64 =
    connection.pragma_query_value(None, "page_size", |row| row.get(0)).map_err(error)?;
  let maximum = (2_i64 * 1024 * 1024 * 1024) / page_size;
  connection.pragma_update(None, "max_page_count", maximum).map_err(error)?;
  let actual: i64 =
    connection.pragma_query_value(None, "max_page_count", |row| row.get(0)).map_err(error)?;
  if actual > maximum {
    return Err("The library index exceeds its 2 GiB disk budget".into());
  }
  connection.execute_batch("CREATE TABLE IF NOT EXISTS metadata (
      id INTEGER PRIMARY KEY CHECK(id=1), account TEXT NOT NULL,
      current_generation INTEGER NOT NULL DEFAULT 0, next_generation INTEGER NOT NULL DEFAULT 1,
      revision INTEGER NOT NULL DEFAULT 0, checked_at INTEGER NOT NULL DEFAULT 0,
      projection_version INTEGER NOT NULL DEFAULT 2);
    CREATE TABLE IF NOT EXISTS artwork (
      reference TEXT PRIMARY KEY, image TEXT, checked_at INTEGER NOT NULL,
      retry_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS award_catalog (
      id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT,
      checked_at INTEGER NOT NULL, retry_at INTEGER NOT NULL,
      revision INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS local_versions (
      name TEXT PRIMARY KEY, version INTEGER NOT NULL CHECK(version>=0),
      applied INTEGER NOT NULL DEFAULT 0 CHECK(applied>=0 AND applied<=version));
    CREATE TABLE IF NOT EXISTS generations (
      id INTEGER PRIMARY KEY, state TEXT NOT NULL, started_at INTEGER NOT NULL, item_count INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS generation_collections (
      generation INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
      collection TEXT NOT NULL, complete INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY(generation,collection));
    CREATE TABLE IF NOT EXISTS generation_revisions (
      generation INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
      name TEXT NOT NULL,stamp INTEGER,
      PRIMARY KEY(generation,name));
    CREATE TABLE IF NOT EXISTS generation_seeds (
      generation INTEGER PRIMARY KEY REFERENCES generations(id) ON DELETE CASCADE,
      source INTEGER NOT NULL, replaced TEXT NOT NULL, after_kind TEXT NOT NULL DEFAULT '',
      after_reference TEXT NOT NULL DEFAULT '', done INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS entities (
      generation INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
      kind TEXT NOT NULL, reference TEXT NOT NULL, canonical_collection TEXT NOT NULL,
      added_key INTEGER NOT NULL, title_key TEXT NOT NULL, search_text TEXT NOT NULL,
      owner TEXT NOT NULL, hires INTEGER NOT NULL, explicit INTEGER NOT NULL,
      PRIMARY KEY(generation,kind,reference)) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS entities_added ON entities(generation,explicit,added_key,kind,reference);
    CREATE INDEX IF NOT EXISTS entities_added_any ON entities(generation,added_key,kind,reference);
    CREATE INDEX IF NOT EXISTS entities_title ON entities(generation,explicit,title_key,kind,reference);
    CREATE INDEX IF NOT EXISTS entities_title_any ON entities(generation,title_key,kind,reference);
    CREATE TABLE IF NOT EXISTS memberships (
      generation INTEGER NOT NULL, kind TEXT NOT NULL, reference TEXT NOT NULL,
      collection TEXT NOT NULL, added_at INTEGER NOT NULL, explicit INTEGER NOT NULL,
      payload TEXT NOT NULL, title_key TEXT NOT NULL, search_text TEXT NOT NULL,
      owner TEXT NOT NULL, hires INTEGER NOT NULL, genres TEXT NOT NULL,
      origin_kind TEXT NOT NULL DEFAULT '', origin_reference TEXT NOT NULL DEFAULT '',
      PRIMARY KEY(generation,kind,reference,collection),
      FOREIGN KEY(generation,kind,reference) REFERENCES entities(generation,kind,reference) ON DELETE CASCADE) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS memberships_lookup ON memberships(generation,collection,kind,reference);
    CREATE TABLE IF NOT EXISTS entity_genres (
      generation INTEGER NOT NULL, kind TEXT NOT NULL, reference TEXT NOT NULL, genre TEXT NOT NULL,
      PRIMARY KEY(generation,kind,reference,genre),
      FOREIGN KEY(generation,kind,reference) REFERENCES entities(generation,kind,reference) ON DELETE CASCADE) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS genres_lookup ON entity_genres(generation,genre,kind,reference);").map_err(error)?;
  add_column(connection, "metadata", "projection_version", "INTEGER NOT NULL DEFAULT 1")?;
  add_column(connection, "memberships", "origin_kind", "TEXT NOT NULL DEFAULT ''")?;
  add_column(connection, "memberships", "origin_reference", "TEXT NOT NULL DEFAULT ''")?;
  connection.pragma_update(None, "user_version", 2).map_err(error)?;
  connection
    .execute("INSERT OR IGNORE INTO metadata(id,account) VALUES(1,?1)", [account.to_string()])
    .map_err(error)?;
  let stored: String = connection
    .query_row("SELECT account FROM metadata WHERE id=1", [], |row| row.get(0))
    .map_err(error)?;
  if stored != account.to_string() {
    return Err("The library index belongs to another account".into());
  }
  // A process restart cannot complete the previous process's unfinished sync.
  connection
    .execute("UPDATE generations SET state='failed' WHERE state='building'", [])
    .map_err(error)?;
  Ok(())
}

fn add_column(
  connection: &Connection,
  table: &str,
  column: &str,
  declaration: &str,
) -> IndexResult<()> {
  let mut statement = connection.prepare(&format!("PRAGMA table_info({table})")).map_err(error)?;
  let mut rows = statement.query([]).map_err(error)?;
  let mut found = false;
  while let Some(row) = rows.next().map_err(error)? {
    if row.get::<_, String>(1).map_err(error)? == column {
      found = true;
      break;
    }
  }
  drop(rows);
  drop(statement);
  if !found {
    connection
      .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"))
      .map_err(error)?;
  }
  Ok(())
}

pub(super) fn initialize_reader(connection: &Connection) -> IndexResult<()> {
  connection.busy_timeout(Duration::from_secs(5)).map_err(error)?;
  connection.set_prepared_statement_cache_capacity(8);
  connection
    .execute_batch(
      "PRAGMA cache_size=-4096; PRAGMA mmap_size=0; PRAGMA temp_store=FILE; PRAGMA query_only=ON;",
    )
    .map_err(error)
}

pub(super) fn status(connection: &Connection, account: u64) -> IndexResult<IndexStatus> {
  connection
    .query_row(
      "SELECT m.current_generation,m.revision,m.checked_at,COALESCE(g.item_count,0)
    FROM metadata m LEFT JOIN generations g ON g.id=m.current_generation WHERE m.id=1",
      [],
      |row| {
        Ok(IndexStatus {
          account,
          generation: row.get(0)?,
          revision: row.get(1)?,
          checked_at: row.get(2)?,
          items: row.get::<_, i64>(3)?.max(0) as u64,
        })
      },
    )
    .map_err(error)
}
