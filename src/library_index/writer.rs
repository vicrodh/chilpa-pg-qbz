use super::*;
use rusqlite::params;

impl LibraryIndex {
  pub fn begin_generation(
    &self,
    collections: &[Membership],
    started_at: i64,
  ) -> IndexResult<Generation> {
    if collections.is_empty() || collections.len() > 4 {
      return Err("A sync must declare between one and four collections".into());
    }
    let mut connection = self.lock();
    let transaction = connection.transaction().map_err(schema::error)?;
    let building: bool = transaction
      .query_row("SELECT EXISTS(SELECT 1 FROM generations WHERE state='building')", [], |row| {
        row.get(0)
      })
      .map_err(schema::error)?;
    if building {
      return Err("A library sync is already in progress".into());
    }
    let obsolete: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM generations WHERE id!=(SELECT current_generation FROM metadata WHERE id=1))", [], |row| row.get(0)).map_err(schema::error)?;
    if obsolete {
      return Err(
        "Obsolete library generations need incremental cleanup before another sync".into(),
      );
    }
    let id: i64 = transaction
      .query_row("SELECT next_generation FROM metadata WHERE id=1", [], |row| row.get(0))
      .map_err(schema::error)?;
    let next = id.checked_add(1).ok_or("The library generation limit was reached")?;
    transaction
      .execute("UPDATE metadata SET next_generation=?1 WHERE id=1", [next])
      .map_err(schema::error)?;
    transaction
      .execute(
        "INSERT INTO generations(id,state,started_at) VALUES(?1,'building',?2)",
        params![id, started_at.max(0)],
      )
      .map_err(schema::error)?;
    for collection in collections {
      transaction
        .execute(
          "INSERT OR IGNORE INTO generation_collections(generation,collection) VALUES(?1,?2)",
          params![id, collection.name()],
        )
        .map_err(schema::error)?;
    }
    transaction.commit().map_err(schema::error)?;
    Ok(Generation { account: self.account, id })
  }

  pub fn ingest(
    &self,
    generation: Generation,
    collection: Membership,
    chunk: &[IndexedItem<'_>],
  ) -> IndexResult<IngestStats> {
    let mut connection = self.lock();
    self.writable(&connection, generation)?;
    let result = (|| {
      if chunk.len() > MAX_CHUNK {
        return Err("The library ingest chunk exceeds 500 entities".into());
      }
      let transaction = connection.transaction().map_err(schema::error)?;
      let seeding: bool = transaction
        .query_row(
          "SELECT EXISTS(SELECT 1 FROM generation_seeds WHERE generation=?1 AND done=0)",
          [generation.id],
          |row| row.get(0),
        )
        .map_err(schema::error)?;
      if seeding {
        return Err("The library snapshot must finish seeding before ingest".into());
      }
      let complete: Option<bool> = transaction
        .query_row(
          "SELECT complete FROM generation_collections WHERE generation=?1 AND collection=?2",
          params![generation.id, collection.name()],
          |row| row.get(0),
        )
        .optional()
        .map_err(schema::error)?;
      if complete != Some(false) {
        return Err("The collection was not declared or has already finished".into());
      }
      let mut inserted = 0_i64;
      let mut new_memberships = 0_u64;
      for entry in chunk {
        let stats =
          super::member::put_member(&transaction, generation, collection.name(), entry, None)?;
        inserted += stats.new_entities as i64;
        new_memberships += stats.new_memberships;
        inserted +=
          super::derived::from_parent(&transaction, generation, collection.name(), entry)? as i64;
      }
      transaction
        .execute(
          "UPDATE generations SET item_count=item_count+?1 WHERE id=?2",
          params![inserted, generation.id],
        )
        .map_err(schema::error)?;
      transaction.commit().map_err(schema::error)?;
      Ok(IngestStats { new_entities: inserted as u64, new_memberships })
    })();
    if result.is_err() {
      // Keep the rejection in memory even when a full disk prevents its durable marker.
      self.failed_generation.store(generation.id, Ordering::Relaxed);
      let _ = connection.execute(
        "UPDATE generations SET state='failed' WHERE id=?1 AND state='building'",
        [generation.id],
      );
    }
    result
  }

  pub fn finish_collection(
    &self,
    generation: Generation,
    collection: Membership,
  ) -> IndexResult<()> {
    let connection = self.lock();
    self.writable(&connection, generation)?;
    let changed = connection
      .execute(
        "UPDATE generation_collections SET complete=1 WHERE generation=?1 AND collection=?2",
        params![generation.id, collection.name()],
      )
      .map_err(schema::error)?;
    if changed == 0 {
      return Err("The collection was not declared for this sync".into());
    }
    Ok(())
  }

  #[cfg(test)]
  pub fn publish(&self, generation: Generation, checked_at: i64) -> IndexResult<IndexStatus> {
    self.publish_with_local_versions(generation, checked_at, &Default::default())
  }

  pub fn publish_with_local_versions(
    &self,
    generation: Generation,
    checked_at: i64,
    captured: &std::collections::BTreeMap<String, u64>,
  ) -> IndexResult<IndexStatus> {
    let mut connection = self.lock();
    self.writable(&connection, generation)?;
    let transaction = connection.transaction().map_err(schema::error)?;
    let pending: bool = transaction
      .query_row(
        "SELECT EXISTS(SELECT 1 FROM generation_collections WHERE generation=?1 AND complete=0)
          OR EXISTS(SELECT 1 FROM generation_seeds WHERE generation=?1 AND done=0)",
        [generation.id],
        |row| row.get(0),
      )
      .map_err(schema::error)?;
    if pending {
      return Err("The library sync still has incomplete collections".into());
    }
    let current: i64 = transaction
      .query_row("SELECT current_generation FROM metadata WHERE id=1", [], |row| row.get(0))
      .map_err(schema::error)?;
    if super::equality::same_snapshot(&transaction, current, generation.id)? {
      transaction
        .execute("DELETE FROM generation_revisions WHERE generation=?1", [current])
        .map_err(schema::error)?;
      transaction
        .execute(
          "INSERT INTO generation_revisions(generation,name,stamp)
        SELECT ?1,name,stamp FROM generation_revisions WHERE generation=?2",
          params![current, generation.id],
        )
        .map_err(schema::error)?;
      transaction
        .execute("UPDATE generations SET state='obsolete' WHERE id=?1", [generation.id])
        .map_err(schema::error)?;
      transaction
        .execute(
          "UPDATE metadata SET checked_at=?1,projection_version=2 WHERE id=1",
          [checked_at.max(0)],
        )
        .map_err(schema::error)?;
      super::local::acknowledge(&transaction, captured)?;
      transaction.commit().map_err(schema::error)?;
      return schema::status(&connection, self.account);
    }
    transaction
      .execute("UPDATE generations SET state='published' WHERE id=?1", [generation.id])
      .map_err(schema::error)?;
    transaction
      .execute(
        "UPDATE metadata SET current_generation=?1,revision=revision+1,checked_at=?2,projection_version=2 WHERE id=1",
        params![generation.id, checked_at.max(0)],
      )
      .map_err(schema::error)?;
    super::local::acknowledge(&transaction, captured)?;
    transaction.commit().map_err(schema::error)?;
    schema::status(&connection, self.account)
  }

  pub fn abandon(&self, generation: Generation) -> IndexResult<()> {
    {
      let connection = self.lock();
      if generation.account != self.account {
        return Err("The generation belongs to another account".into());
      }
      connection
        .execute(
          "UPDATE generations SET state='failed' WHERE id=?1 AND state='building'",
          [generation.id],
        )
        .map_err(schema::error)?;
    }
    Ok(())
  }
}
