use super::*;
use rusqlite::params;
use std::collections::BTreeMap;

fn valid_name(name: &str) -> bool {
  !name.is_empty()
    && name.len() <= 64
    && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

impl LibraryIndex {
  pub fn mark_local_changed(&self, name: &str) -> IndexResult<u64> {
    if !valid_name(name) {
      return Err("The local library source name is invalid".into());
    }
    let mut connection = self.lock();
    let transaction = connection.transaction().map_err(schema::error)?;
    let previous: Option<i64> = transaction
      .query_row("SELECT version FROM local_versions WHERE name=?1", [name], |row| row.get(0))
      .optional()
      .map_err(schema::error)?;
    if previous.is_none() {
      let count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM local_versions", [], |row| row.get(0))
        .map_err(schema::error)?;
      if count >= 32 {
        return Err("The local library source limit was reached".into());
      }
    }
    let version =
      previous.unwrap_or(0).checked_add(1).ok_or("The local library version limit was reached")?;
    transaction
      .execute(
        "INSERT INTO local_versions(name,version,applied) VALUES(?1,?2,0)
        ON CONFLICT(name) DO UPDATE SET version=excluded.version",
        params![name, version],
      )
      .map_err(schema::error)?;
    transaction.commit().map_err(schema::error)?;
    Ok(version as u64)
  }

  pub fn local_versions(&self) -> IndexResult<LocalVersions> {
    let connection = self.read();
    let mut statement = connection
      .prepare_cached("SELECT name,version,applied FROM local_versions ORDER BY name LIMIT 33")
      .map_err(schema::error)?;
    let rows = statement
      .query_map([], |row| {
        let version: i64 = row.get(1)?;
        let applied: i64 = row.get(2)?;
        Ok((
          row.get::<_, String>(0)?,
          LocalVersion { version: version as u64, applied: applied as u64 },
        ))
      })
      .map_err(schema::error)?;
    let versions = rows.collect::<Result<LocalVersions, _>>().map_err(schema::error)?;
    if versions.len() > 32 {
      return Err("The local library source limit was reached".into());
    }
    Ok(versions)
  }
}

pub(super) fn acknowledge(
  connection: &Connection,
  captured: &BTreeMap<String, u64>,
) -> IndexResult<()> {
  if captured.len() > 32
    || captured.iter().any(|(name, version)| !valid_name(name) || *version > i64::MAX as u64)
  {
    return Err("The captured local library versions are invalid".into());
  }
  for (name, version) in captured {
    let current: Option<i64> = connection
      .query_row("SELECT version FROM local_versions WHERE name=?1", [name], |row| row.get(0))
      .optional()
      .map_err(schema::error)?;
    if *version > current.unwrap_or(0) as u64 {
      return Err("The captured local library version exceeds its durable version".into());
    }
    connection
      .execute(
        "UPDATE local_versions SET applied=MAX(applied,?2) WHERE name=?1",
        params![name, *version as i64],
      )
      .map_err(schema::error)?;
  }
  Ok(())
}
