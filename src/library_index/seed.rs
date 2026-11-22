use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, params_from_iter, types::Value as SqlValue};

#[derive(Serialize, Deserialize)]
struct SeedCursor {
  version: u8,
  account: u64,
  generation: i64,
  source: i64,
  kind: String,
  reference: String,
}

struct SeedState {
  source: i64,
  replaced: String,
  kind: String,
  reference: String,
  done: bool,
}

fn replacements(replace: &[(Membership, String)]) -> IndexResult<Vec<(Membership, String)>> {
  if replace.len() > 32
    || replace.iter().any(|(_, kind)| {
      !matches!(kind.as_str(), "album" | "track" | "artist" | "label" | "award" | "playlist")
    })
  {
    return Err("The replaced library collections are invalid".into());
  }
  let mut replace = replace.to_vec();
  replace.sort();
  replace.dedup();
  Ok(replace)
}

fn boundary(
  connection: &Connection,
  state: &SeedState,
  limit: usize,
) -> IndexResult<Option<(String, String, bool)>> {
  let mut statement = connection
    .prepare_cached(
      "SELECT kind,reference FROM entities WHERE generation=?1 AND (kind,reference)>(?2,?3)
      ORDER BY kind,reference LIMIT ?4",
    )
    .map_err(schema::error)?;
  let mut rows = statement
    .query(params![state.source, state.kind, state.reference, (limit + 1) as i64])
    .map_err(schema::error)?;
  let mut last = None;
  let mut count = 0;
  while let Some(row) = rows.next().map_err(schema::error)? {
    if count == limit {
      return Ok(last.map(|(kind, reference)| (kind, reference, true)));
    }
    last = Some((
      row.get::<_, String>(0).map_err(schema::error)?,
      row.get::<_, String>(1).map_err(schema::error)?,
    ));
    count += 1;
  }
  Ok(last.map(|(kind, reference)| (kind, reference, false)))
}

struct CopyStatements {
  entities: String,
  memberships: String,
  arguments: Vec<SqlValue>,
}

const COPY_GENRES: &str = "INSERT INTO entity_genres(generation,kind,reference,genre)
  SELECT e.generation,e.kind,e.reference,g.value FROM entities e CROSS JOIN memberships m
    ON m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference AND m.collection=e.canonical_collection
    JOIN json_each(m.genres) g WHERE e.generation=?1 AND (e.kind,e.reference) IN (
      SELECT c.kind,c.reference FROM entities c WHERE c.generation=?2
        AND (c.kind,c.reference)>(?3,?4) AND (c.kind,c.reference)<=(?5,?6))";

fn copy_statements(
  generation: Generation,
  state: &SeedState,
  replace: &[(Membership, String)],
  last: &(String, String),
) -> CopyStatements {
  let placeholders =
    if replace.is_empty() { "(NULL,NULL)".into() } else { vec!["(?,?)"; replace.len()].join(",") };
  let cte = format!("WITH replaced(collection,kind) AS (VALUES {placeholders}) ");
  let remaining =
    "m.collection NOT LIKE 'derived:%' AND NOT EXISTS(SELECT 1 FROM replaced r WHERE r.collection=m.collection AND r.kind=m.kind)";
  let mut arguments: Vec<SqlValue> = replace
    .iter()
    .flat_map(|(collection, kind)| {
      [SqlValue::Text(collection.name().into()), SqlValue::Text(kind.clone())]
    })
    .collect();
  arguments.extend([
    SqlValue::Integer(generation.id),
    SqlValue::Integer(state.source),
    SqlValue::Text(state.kind.clone()),
    SqlValue::Text(state.reference.clone()),
    SqlValue::Text(last.0.clone()),
    SqlValue::Text(last.1.clone()),
  ]);
  let canonical = format!("SELECT choice.collection FROM memberships choice
    WHERE choice.generation=e.generation AND choice.kind=e.kind AND choice.reference=e.reference
    AND {} ORDER BY explicit DESC,CASE choice.collection WHEN 'purchases' THEN 3 WHEN 'subscribed' THEN 2 WHEN 'followed' THEN 1 ELSE 0 END DESC LIMIT 1",
    remaining.replace("m.", "choice."));
  let entities = format!("{cte}INSERT OR IGNORE INTO entities(generation,kind,reference,canonical_collection,added_key,title_key,search_text,owner,hires,explicit)
    SELECT ?,e.kind,e.reference,m.collection,
    (SELECT -COALESCE(MAX(CASE WHEN m.explicit=1 THEN m.added_at END),MAX(m.added_at)) FROM memberships m WHERE m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference AND {remaining}),
    m.title_key,m.search_text,m.owner,m.hires,
    (SELECT MAX(m.explicit) FROM memberships m WHERE m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference AND {remaining})
    FROM entities e CROSS JOIN memberships m ON m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference
      AND m.collection=({canonical}) WHERE e.generation=? AND (e.kind,e.reference)>(?,?) AND (e.kind,e.reference)<=(?,?)");
  let memberships = format!("{cte}INSERT INTO memberships(generation,kind,reference,collection,added_at,explicit,payload,title_key,search_text,owner,hires,genres,origin_kind,origin_reference)
    SELECT ?,m.kind,m.reference,m.collection,m.added_at,m.explicit,m.payload,m.title_key,m.search_text,m.owner,m.hires,m.genres,m.origin_kind,m.origin_reference
    FROM memberships m WHERE m.generation=? AND (m.kind,m.reference)>(?,?) AND (m.kind,m.reference)<=(?,?) AND {remaining}");
  CopyStatements { entities, memberships, arguments }
}

fn copy_chunk(
  connection: &Connection,
  generation: Generation,
  state: &SeedState,
  replace: &[(Membership, String)],
  last: &(String, String),
) -> IndexResult<IngestStats> {
  // Keep the bounded entity range outermost when SQLite chooses the join order.
  let sql = copy_statements(generation, state, replace, last);
  let mut new_entities = connection
    .execute(&sql.entities, params_from_iter(sql.arguments.iter()))
    .map_err(schema::error)? as u64;
  let new_memberships = connection
    .execute(&sql.memberships, params_from_iter(sql.arguments.iter()))
    .map_err(schema::error)? as u64;
  super::member::refresh_candidates(
    connection,
    generation,
    state.source,
    (&state.kind, &state.reference),
    (&last.0, &last.1),
  )?;
  connection
    .execute(
      COPY_GENRES,
      params![generation.id, state.source, state.kind, state.reference, last.0, last.1],
    )
    .map_err(schema::error)?;
  new_entities += super::derived::rebuild_parents(
    connection,
    generation,
    state.source,
    (&state.kind, &state.reference),
    (&last.0, &last.1),
  )?;
  connection
    .execute(
      "UPDATE generations SET item_count=item_count+?1 WHERE id=?2",
      params![new_entities as i64, generation.id],
    )
    .map_err(schema::error)?;
  Ok(IngestStats { new_entities, new_memberships })
}

impl LibraryIndex {
  #[cfg(test)]
  pub(super) fn explain_seed_chunk(
    &self,
    generation: Generation,
    replace: &[(Membership, String)],
    after: (&str, &str),
    through: (&str, &str),
  ) -> IndexResult<[Vec<String>; 3]> {
    let connection = self.read();
    let state = SeedState {
      source: schema::status(&connection, self.account)?.generation,
      replaced: String::new(),
      kind: after.0.into(),
      reference: after.1.into(),
      done: false,
    };
    let sql = copy_statements(
      generation,
      &state,
      &replacements(replace)?,
      &(through.0.into(), through.1.into()),
    );
    let explain = |sql: &str, arguments: &[SqlValue]| -> IndexResult<Vec<String>> {
      let mut statement =
        connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).map_err(schema::error)?;
      let rows = statement
        .query_map(params_from_iter(arguments.iter()), |row| row.get(3))
        .map_err(schema::error)?;
      rows.collect::<Result<Vec<_>, _>>().map_err(schema::error)
    };
    Ok([
      explain(&sql.entities, &sql.arguments)?,
      explain(&sql.memberships, &sql.arguments)?,
      explain(
        COPY_GENRES,
        &[
          SqlValue::Integer(generation.id),
          SqlValue::Integer(state.source),
          SqlValue::Text(after.0.into()),
          SqlValue::Text(after.1.into()),
          SqlValue::Text(through.0.into()),
          SqlValue::Text(through.1.into()),
        ],
      )?,
    ])
  }

  pub fn seed_from_current_chunk(
    &self,
    generation: Generation,
    replace: &[(Membership, String)],
    encoded_cursor: Option<&str>,
    limit: usize,
  ) -> IndexResult<SeedProgress> {
    let mut connection = self.lock();
    self.writable(&connection, generation)?;
    let result = (|| {
      let replace = replacements(replace)?;
      let replaced =
        serde_json::to_string(&replace).map_err(|_| "The seed collections could not be encoded")?;
      let cursor: Option<SeedCursor> = encoded_cursor
        .map(|encoded| {
          if encoded.len() > 16384 {
            return Err("The library seed cursor exceeds its limit");
          }
          let bytes =
            URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "The library seed cursor is invalid")?;
          serde_json::from_slice(&bytes).map_err(|_| "The library seed cursor is invalid")
        })
        .transpose()?;
      let transaction = connection.transaction().map_err(schema::error)?;
      let current: i64 = transaction
        .query_row("SELECT current_generation FROM metadata WHERE id=1", [], |row| row.get(0))
        .map_err(schema::error)?;
      let stored = transaction.query_row("SELECT source,replaced,after_kind,after_reference,done FROM generation_seeds WHERE generation=?1", [generation.id], |row| {
        Ok(SeedState { source: row.get(0)?, replaced: row.get(1)?, kind: row.get(2)?, reference: row.get(3)?, done: row.get(4)? })
      }).optional().map_err(schema::error)?;
      let state = if let Some(state) = stored {
        state
      } else {
        let empty: bool = transaction
          .query_row("SELECT item_count=0 FROM generations WHERE id=?1", [generation.id], |row| {
            row.get(0)
          })
          .map_err(schema::error)?;
        if !empty || cursor.is_some() {
          return Err("Only an empty library generation can start seeding".into());
        }
        transaction
          .execute(
            "INSERT INTO generation_seeds(generation,source,replaced) VALUES(?1,?2,?3)",
            params![generation.id, current, replaced],
          )
          .map_err(schema::error)?;
        SeedState {
          source: current,
          replaced: replaced.clone(),
          kind: String::new(),
          reference: String::new(),
          done: false,
        }
      };
      if state.done || state.source != current || state.replaced != replaced {
        return Err("The library seed belongs to another snapshot or collection selection".into());
      }
      let valid_cursor = match cursor {
        Some(cursor) => {
          cursor.version == 1
            && cursor.account == self.account
            && cursor.generation == generation.id
            && cursor.source == state.source
            && cursor.kind == state.kind
            && cursor.reference == state.reference
        }
        None => state.kind.is_empty() && state.reference.is_empty(),
      };
      if !valid_cursor {
        return Err("The library seed cursor did not advance from its last chunk".into());
      }
      let last = boundary(&transaction, &state, limit.clamp(1, MAX_CHUNK))?;
      let (stats, next, done) = if let Some((kind, reference, more)) = last {
        let stats = copy_chunk(
          &transaction,
          generation,
          &state,
          &replace,
          &(kind.clone(), reference.clone()),
        )?;
        transaction.execute("UPDATE generation_seeds SET after_kind=?1,after_reference=?2,done=?3 WHERE generation=?4", params![kind,reference,!more,generation.id]).map_err(schema::error)?;
        let next = if more {
          Some(
            URL_SAFE_NO_PAD.encode(
              serde_json::to_vec(&SeedCursor {
                version: 1,
                account: self.account,
                generation: generation.id,
                source: state.source,
                kind,
                reference,
              })
              .map_err(|_| "The library seed cursor could not be encoded")?,
            ),
          )
        } else {
          None
        };
        (stats, next, !more)
      } else {
        transaction
          .execute("UPDATE generation_seeds SET done=1 WHERE generation=?1", [generation.id])
          .map_err(schema::error)?;
        (IngestStats::default(), None, true)
      };
      transaction.commit().map_err(schema::error)?;
      Ok(SeedProgress { stats, next, done })
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

  #[cfg(test)]
  pub fn seed_from_current(
    &self,
    generation: Generation,
    replace: &[(Membership, String)],
  ) -> IndexResult<IngestStats> {
    let mut cursor = None;
    let mut total = IngestStats::default();
    loop {
      let progress =
        self.seed_from_current_chunk(generation, replace, cursor.as_deref(), MAX_CHUNK)?;
      total.new_entities += progress.stats.new_entities;
      total.new_memberships += progress.stats.new_memberships;
      if progress.done {
        return Ok(total);
      }
      cursor = progress.next;
    }
  }
}
