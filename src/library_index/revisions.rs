use super::*;
use rusqlite::params;

impl LibraryIndex {
  pub fn projection_pending(&self) -> IndexResult<bool> {
    self
      .read()
      .query_row("SELECT projection_version<2 FROM metadata WHERE id=1", [], |row| row.get(0))
      .map_err(schema::error)
  }

  pub fn revisions(&self) -> IndexResult<Revisions> {
    let connection = self.read();
    let mut statement = connection
      .prepare_cached(
        "SELECT r.name,r.stamp FROM generation_revisions r
      JOIN metadata m ON m.current_generation=r.generation WHERE m.id=1 ORDER BY r.name",
      )
      .map_err(schema::error)?;
    let rows = statement
      .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))
      .map_err(schema::error)?;
    rows.collect::<Result<Revisions, _>>().map_err(schema::error)
  }

  pub fn mark_checked(&self, checked_at: i64) -> IndexResult<()> {
    self
      .lock()
      .execute("UPDATE metadata SET checked_at=?1 WHERE id=1", [checked_at.max(0)])
      .map_err(schema::error)?;
    Ok(())
  }

  pub fn stage_revisions(&self, generation: Generation, revisions: &Revisions) -> IndexResult<()> {
    let mut connection = self.lock();
    self.writable(&connection, generation)?;
    let result = (|| {
      if revisions.len() > 32
        || revisions.iter().any(|(name, stamp)| {
          name.is_empty()
            || name.len() > 64
            || !name
              .bytes()
              .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            || stamp.is_some_and(|stamp| stamp < 0)
        })
      {
        return Err("The library revision metadata is invalid".into());
      }
      let transaction = connection.transaction().map_err(schema::error)?;
      transaction
        .execute("DELETE FROM generation_revisions WHERE generation=?1", [generation.id])
        .map_err(schema::error)?;
      for (name, stamp) in revisions {
        transaction
          .prepare_cached(
            "INSERT INTO generation_revisions(generation,name,stamp) VALUES(?1,?2,?3)",
          )
          .map_err(schema::error)?
          .execute(params![generation.id, name, stamp])
          .map_err(schema::error)?;
      }
      transaction.commit().map_err(schema::error)
    })();
    if result.is_err() {
      self.failed_generation.store(generation.id, Ordering::Relaxed);
      let _ = connection.execute(
        "UPDATE generations SET state='failed' WHERE id=?1 AND state='building'",
        [generation.id],
      );
    }
    result
  }
}
