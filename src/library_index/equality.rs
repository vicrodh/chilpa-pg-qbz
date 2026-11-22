use super::*;

fn same_rows(
  connection: &Connection,
  before: i64,
  after: i64,
  table: &str,
  columns: &[&str],
  order: &str,
) -> IndexResult<bool> {
  let sql =
    format!("SELECT {} FROM {table} WHERE generation=?1 ORDER BY {order}", columns.join(","));
  let mut old_statement = connection.prepare(&sql).map_err(schema::error)?;
  let mut new_statement = connection.prepare(&sql).map_err(schema::error)?;
  let mut old_rows = old_statement.query([before]).map_err(schema::error)?;
  let mut new_rows = new_statement.query([after]).map_err(schema::error)?;
  loop {
    match (old_rows.next().map_err(schema::error)?, new_rows.next().map_err(schema::error)?) {
      (None, None) => return Ok(true),
      (Some(old), Some(new)) => {
        for column in 0..columns.len() {
          if old.get_ref(column).map_err(schema::error)?
            != new.get_ref(column).map_err(schema::error)?
          {
            return Ok(false);
          }
        }
      }
      _ => return Ok(false),
    }
  }
}

pub(super) fn same_snapshot(connection: &Connection, before: i64, after: i64) -> IndexResult<bool> {
  if before == 0 {
    return Ok(false);
  }
  let counts: (i64, i64) = connection
    .query_row(
      "SELECT old.item_count,new.item_count
    FROM generations old JOIN generations new ON new.id=?2 WHERE old.id=?1",
      rusqlite::params![before, after],
      |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(schema::error)?;
  if counts.0 != counts.1 {
    return Ok(false);
  }
  // Ordered primary-key cursors compare exact values with two live SQLite rows, not JSON copies.
  Ok(
    same_rows(
      connection,
      before,
      after,
      "entities",
      &[
        "kind",
        "reference",
        "canonical_collection",
        "added_key",
        "title_key",
        "search_text",
        "owner",
        "hires",
        "explicit",
      ],
      "kind,reference",
    )? && same_rows(
      connection,
      before,
      after,
      "memberships",
      &[
        "kind",
        "reference",
        "collection",
        "added_at",
        "explicit",
        "payload",
        "title_key",
        "search_text",
        "owner",
        "hires",
        "genres",
        "origin_kind",
        "origin_reference",
      ],
      "kind,reference,collection",
    )? && same_rows(
      connection,
      before,
      after,
      "entity_genres",
      &["kind", "reference", "genre"],
      "kind,reference,genre",
    )?,
  )
}
