use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params_from_iter, types::Value as SqlValue};

#[derive(Serialize, Deserialize)]
struct Cursor {
  version: u8,
  account: u64,
  generation: i64,
  query: String,
  key: SortKey,
  kind: String,
  reference: String,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SortKey {
  Added(i64),
  Title(String),
}

fn values(values: &mut Vec<String>, lowercase: bool) -> IndexResult<()> {
  if values.len() > 64 || values.iter().any(|value| value.len() > 512) {
    return Err("The library filter exceeds its option limit".into());
  }
  for value in values.iter_mut() {
    *value = if lowercase { value.trim().to_lowercase() } else { value.trim().to_owned() };
  }
  values.retain(|value| !value.is_empty());
  values.sort();
  values.dedup();
  Ok(())
}

fn normalized(query: &Query) -> IndexResult<Query> {
  let mut query = query.clone();
  if query.text.len() > 256 || query.membership_groups.len() > 4 {
    return Err("The library query exceeds its limit".into());
  }
  values(&mut query.kinds, true)?;
  values(&mut query.genres, true)?;
  values(&mut query.owners, false)?;
  values(&mut query.exclude_owners, false)?;
  for group in &mut query.membership_groups {
    if group.len() > 4 {
      return Err("The library membership filter exceeds its limit".into());
    }
    group.sort();
    group.dedup();
  }
  query.membership_groups.retain(|group| !group.is_empty());
  query.membership_groups.sort();
  query.membership_groups.dedup();
  let words: Vec<_> = query.text.split_whitespace().take(17).collect();
  if words.len() > 16 {
    return Err("The library text filter exceeds sixteen words".into());
  }
  query.text = words.join(" ").to_lowercase();
  Ok(query)
}

fn signature(query: &Query) -> IndexResult<String> {
  let encoded = serde_json::to_vec(query).map_err(|_| "The library query could not be encoded")?;
  let (mut first, mut second) = (0xcbf29ce484222325_u64, 0x84222325cbf29ce4_u64);
  for byte in encoded {
    first = (first ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    second = (second ^ u64::from(byte)).wrapping_mul(0x100000001b3);
  }
  Ok(format!("{first:016x}{second:016x}"))
}

fn cursor(
  encoded: Option<&str>,
  status: &IndexStatus,
  signature: &str,
  order: Order,
) -> IndexResult<Option<Cursor>> {
  let Some(encoded) = encoded else {
    return Ok(None);
  };
  if encoded.len() > 16384 {
    return Err("The library cursor exceeds its limit".into());
  }
  let decoded = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "The library cursor is invalid")?;
  let cursor: Cursor =
    serde_json::from_slice(&decoded).map_err(|_| "The library cursor is invalid")?;
  if cursor.version != 1 || cursor.account != status.account || cursor.query != signature {
    return Err("The library cursor belongs to another query or account".into());
  }
  if cursor.generation != status.generation {
    return Err("The library cursor is stale; reload the first page".into());
  }
  if !matches!(
    (&cursor.key, order),
    (SortKey::Added(_), Order::Added) | (SortKey::Title(_), Order::Title)
  ) {
    return Err("The library cursor has an invalid ordering key".into());
  }
  if cursor.kind.len() > 32
    || cursor.reference.len() > 2048
    || matches!(&cursor.key, SortKey::Title(title) if title.len() > 2048)
  {
    return Err("The library cursor exceeds its key limits".into());
  }
  Ok(Some(cursor))
}

struct Statement {
  sql: String,
  arguments: Vec<SqlValue>,
}
impl Statement {
  fn options(&mut self, column: &str, options: &[String]) {
    if options.is_empty() {
      return;
    }
    self.sql.push_str(&format!(" AND {column} IN ({})", vec!["?"; options.len()].join(",")));
    self.arguments.extend(options.iter().cloned().map(SqlValue::Text));
  }
}

fn statement(query: &Query, generation: i64, cursor: Option<&Cursor>, limit: usize) -> Statement {
  let mut result = Statement { sql: "SELECT (SELECT payload FROM memberships card WHERE card.generation=e.generation
      AND card.kind=e.kind AND card.reference=e.reference AND card.collection=e.canonical_collection),e.added_key,e.title_key,e.kind,e.reference,
    (SELECT json_group_array(json_object('collection',CASE WHEN collection LIKE 'derived:%' THEN substr(collection,9) ELSE collection END,'added_at',added_at,'explicit',explicit))
      FROM (SELECT collection,added_at,explicit FROM memberships m WHERE m.generation=e.generation
        AND m.kind=e.kind AND m.reference=e.reference ORDER BY collection))
    FROM entities e WHERE e.generation=?".into(), arguments: vec![SqlValue::Integer(generation)] };
  if query.explicit_only {
    result.sql.push_str(" AND e.explicit=1");
  }
  result.options("e.kind", &query.kinds);
  result.options("e.owner", &query.owners);
  if !query.exclude_owners.is_empty() {
    result.sql.push_str(&format!(
      " AND e.owner NOT IN ({})",
      vec!["?"; query.exclude_owners.len()].join(",")
    ));
    result.arguments.extend(query.exclude_owners.iter().cloned().map(SqlValue::Text));
  }
  if let Some(hires) = query.hires {
    result.sql.push_str(" AND e.hires=?");
    result.arguments.push(SqlValue::Integer(i64::from(hires)));
  }
  if !query.genres.is_empty() {
    result.sql.push_str(" AND EXISTS(SELECT 1 FROM entity_genres g WHERE g.generation=e.generation AND g.kind=e.kind AND g.reference=e.reference");
    result.options("g.genre", &query.genres);
    result.sql.push(')');
  }
  for group in &query.membership_groups {
    result.sql.push_str(" AND EXISTS(SELECT 1 FROM memberships m WHERE m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference");
    if query.explicit_only {
      result.sql.push_str(" AND m.explicit=1");
    } else if group.contains(&Membership::Followed) {
      result.sql.push_str(" AND (m.collection!='followed' OR m.explicit=1)");
    }
    result.options(
      "m.collection",
      &group
        .iter()
        .flat_map(|value| {
          let name = value.name().to_owned();
          let derived = (!query.explicit_only
            && matches!(value, Membership::Favorites | Membership::Purchases))
          .then(|| format!("derived:{name}"));
          std::iter::once(name).chain(derived)
        })
        .collect::<Vec<_>>(),
    );
    result.sql.push(')');
  }
  if query.exclude_purchases {
    result.sql.push_str(" AND NOT EXISTS(SELECT 1 FROM memberships p WHERE p.generation=e.generation AND p.kind=e.kind AND p.reference=e.reference");
    result.sql.push_str(if query.explicit_only {
      " AND p.collection='purchases' AND p.explicit=1)"
    } else {
      " AND p.collection IN ('purchases','derived:purchases'))"
    });
  }
  for word in query.text.split_whitespace() {
    let escaped = word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    result.sql.push_str(" AND e.search_text LIKE ? ESCAPE '\\'");
    result.arguments.push(SqlValue::Text(format!("%{escaped}%")));
  }
  let key = if query.order == Order::Added { "added_key" } else { "title_key" };
  if let Some(cursor) = cursor {
    result.sql.push_str(&format!(" AND (e.{key},e.kind,e.reference)>(?,?,?)"));
    result.arguments.push(match &cursor.key {
      SortKey::Added(value) => SqlValue::Integer(*value),
      SortKey::Title(value) => SqlValue::Text(value.clone()),
    });
    result.arguments.push(SqlValue::Text(cursor.kind.clone()));
    result.arguments.push(SqlValue::Text(cursor.reference.clone()));
  }
  result.sql.push_str(&format!(" ORDER BY e.{key},e.kind,e.reference LIMIT ?"));
  result.arguments.push(SqlValue::Integer((limit.clamp(1, MAX_PAGE) + 1) as i64));
  result
}

fn genre_presence(generation: i64, genres: &[String]) -> Statement {
  let mut presence = Statement {
    sql: "SELECT EXISTS(SELECT 1 FROM entity_genres INDEXED BY genres_lookup WHERE generation=?"
      .into(),
    arguments: vec![SqlValue::Integer(generation)],
  };
  presence.options("genre", genres);
  presence.sql.push_str(" LIMIT 1)");
  presence
}

impl LibraryIndex {
  pub fn query(
    &self,
    query: &Query,
    encoded_cursor: Option<&str>,
    limit: usize,
  ) -> IndexResult<Page> {
    let query = normalized(query)?;
    let signature = signature(&query)?;
    let limit = limit.clamp(1, MAX_PAGE);
    let mut reader = self.read();
    let snapshot = reader.transaction().map_err(schema::error)?;
    let connection = &*snapshot;
    let status = schema::status(&connection, self.account)?;
    let cursor = cursor(encoded_cursor, &status, &signature, query.order)?;
    if !query.genres.is_empty() {
      let presence = genre_presence(status.generation, &query.genres);
      let found: bool = connection
        .prepare_cached(&presence.sql)
        .map_err(schema::error)?
        .query_row(params_from_iter(presence.arguments), |row| row.get(0))
        .map_err(schema::error)?;
      if !found {
        return Ok(Page { items: Vec::new(), next: None, has_more: false, status });
      }
    }
    let sql = statement(&query, status.generation, cursor.as_ref(), limit);
    let mut statement = connection.prepare_cached(&sql.sql).map_err(schema::error)?;
    let mut rows = statement.query(params_from_iter(sql.arguments)).map_err(schema::error)?;
    let mut items = Vec::with_capacity(limit);
    let mut last = None;
    let mut has_more = false;
    while let Some(row) = rows.next().map_err(schema::error)? {
      if items.len() == limit {
        has_more = true;
        break;
      }
      let payload: String = row.get(0).map_err(schema::error)?;
      let mut item: Value =
        serde_json::from_str(&payload).map_err(|_| "The indexed library entity is unreadable")?;
      let memberships: String = row.get(5).map_err(schema::error)?;
      let mut memberships: Value = serde_json::from_str(&memberships)
        .map_err(|_| "The indexed collection flags are unreadable")?;
      let mut explicit = false;
      for field in ["favorite", "purchased", "followed", "subscribed"] {
        item[field] = false.into();
      }
      for member in memberships.as_array_mut().into_iter().flatten() {
        let marked = member["explicit"].as_i64() == Some(1);
        member["explicit"] = marked.into();
        explicit |= marked;
        if let Some(field) = match member["collection"].as_str().filter(|_| marked) {
          Some("favorites") => Some("favorite"),
          Some("purchases") => Some("purchased"),
          Some("followed") => Some("followed"),
          Some("subscribed") => Some("subscribed"),
          _ => None,
        } {
          item[field] = true.into();
        }
      }
      if matches!(item["kind"].as_str(), Some("artist" | "label" | "award")) {
        item["favorite"] = item["followed"].clone();
      }
      item["membership"] = memberships
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|member| member["collection"].as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",")
        .into();
      item["collections"] = memberships;
      item["library_explicit"] = explicit.into();
      item["inferred"] = (!explicit).into();
      item["in_library"] = true.into();
      let added_key: i64 = row.get(1).map_err(schema::error)?;
      item["added_at"] = (-added_key).into();
      last = Some(Cursor {
        version: 1,
        account: self.account,
        generation: status.generation,
        query: signature.clone(),
        key: if query.order == Order::Added {
          SortKey::Added(added_key)
        } else {
          SortKey::Title(row.get(2).map_err(schema::error)?)
        },
        kind: row.get(3).map_err(schema::error)?,
        reference: row.get(4).map_err(schema::error)?,
      });
      items.push(item);
    }
    let next = if has_more {
      Some(
        URL_SAFE_NO_PAD.encode(
          serde_json::to_vec(&last.ok_or("The library page has no continuation key")?)
            .map_err(|_| "The library cursor could not be encoded")?,
        ),
      )
    } else {
      None
    };
    Ok(Page { items, next, has_more, status })
  }

  #[cfg(test)]
  pub(super) fn explain_genre_presence(&self, genres: &[String]) -> IndexResult<Vec<String>> {
    let connection = self.read();
    let status = schema::status(&connection, self.account)?;
    let sql = genre_presence(status.generation, genres);
    let mut statement =
      connection.prepare(&format!("EXPLAIN QUERY PLAN {}", sql.sql)).map_err(schema::error)?;
    let result = statement
      .query_map(params_from_iter(sql.arguments), |row| row.get(3))
      .map_err(schema::error)?
      .collect::<Result<Vec<_>, _>>()
      .map_err(schema::error)?;
    Ok(result)
  }

  #[cfg(test)]
  pub(super) fn explain(
    &self,
    query: &Query,
    encoded_cursor: Option<&str>,
    limit: usize,
  ) -> IndexResult<Vec<String>> {
    let query = normalized(query)?;
    let mut reader = self.read();
    let snapshot = reader.transaction().map_err(schema::error)?;
    let connection = &*snapshot;
    let status = schema::status(&connection, self.account)?;
    let cursor = cursor(encoded_cursor, &status, &signature(&query)?, query.order)?;
    let sql = statement(&query, status.generation, cursor.as_ref(), limit);
    let mut statement =
      connection.prepare(&format!("EXPLAIN QUERY PLAN {}", sql.sql)).map_err(schema::error)?;
    let result = statement
      .query_map(params_from_iter(sql.arguments), |row| row.get(3))
      .map_err(schema::error)?
      .collect::<Result<Vec<_>, _>>()
      .map_err(schema::error)?;
    Ok(result)
  }
}
