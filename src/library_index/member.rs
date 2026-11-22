use super::*;
use rusqlite::params;
use std::{collections::BTreeSet, io::Write};

struct Payload(Vec<u8>);
impl Write for Payload {
  fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
    if self.0.len().saturating_add(bytes.len()) > MAX_PAYLOAD {
      return Err(std::io::Error::other("Library entity exceeds 64 KiB"));
    }
    self.0.extend_from_slice(bytes);
    Ok(bytes.len())
  }
  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

fn payload(item: &Value) -> IndexResult<String> {
  let mut bytes = Payload(Vec::with_capacity(4096));
  serde_json::to_writer(&mut bytes, item)
    .map_err(|_| "The library entity cannot be indexed within 64 KiB")?;
  String::from_utf8(bytes.0).map_err(|_| "The library entity is not valid text".into())
}

fn identity(item: &Value) -> IndexResult<(&str, &str)> {
  let kind = item["kind"]
    .as_str()
    .filter(|kind| matches!(*kind, "album" | "track" | "artist" | "label" | "award" | "playlist"))
    .ok_or("The library entity has an unsupported kind")?;
  let reference = item["ref"]
    .as_str()
    .filter(|value| {
      !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
    })
    .ok_or("The library entity has an invalid reference")?;
  Ok((kind, reference))
}

fn owner(item: &Value) -> String {
  let value = item.get("owner_id").or_else(|| item.pointer("/owner/id"));
  match value {
    Some(Value::String(value)) => value.clone(),
    Some(Value::Number(value)) => value.to_string(),
    _ => String::new(),
  }
}

fn hires(item: &Value) -> bool {
  let quality = &item["quality"];
  item["hires"] == true
    || quality["tier"] == "hi_res"
    || quality["bits"].as_u64().is_some_and(|bits| bits > 16)
    || quality["rate"].as_u64().is_some_and(|rate| rate > 48000)
}

fn genres(item: &Value) -> IndexResult<BTreeSet<String>> {
  let mut found = BTreeSet::new();
  let sources = item["genres"]
    .as_array()
    .into_iter()
    .flatten()
    .chain(std::iter::once(&item["genre"]))
    .chain(item["genre_ids"].as_array().into_iter().flatten())
    .chain(std::iter::once(&item["genre_id"]));
  for value in sources {
    for value in [value, &value["id"], &value["name"]] {
      let name = match value {
        Value::String(name) => name.trim().to_lowercase(),
        Value::Number(number) => number.to_string(),
        _ => continue,
      };
      if name.is_empty() {
        continue;
      }
      if name.len() > 512 || !found.contains(&name) && found.len() >= 64 {
        return Err("The library entity exceeds its genre metadata limit".into());
      }
      found.insert(name);
    }
  }
  Ok(found)
}

pub(super) fn put_member(
  connection: &Connection,
  generation: Generation,
  collection: &str,
  entry: &IndexedItem<'_>,
  origin: Option<(&str, &str)>,
) -> IndexResult<IngestStats> {
  let (kind, reference) = identity(entry.item)?;
  // Bound the incoming representation before cloning or merging it.
  payload(entry.item)?;
  let mut item = Value::Object(Default::default());
  for (key, value) in entry.item.as_object().ok_or("The library entity must be an object")? {
    if !value.is_null()
      && !matches!(
        key.as_str(),
        "favorite"
          | "purchased"
          | "followed"
          | "subscribed"
          | "membership"
          | "collections"
          | "library_explicit"
          | "in_library"
          | "added_at"
      )
    {
      item[key] = value.clone();
    }
  }
  let title = item["title"].as_str().unwrap_or_default().to_lowercase();
  if title.len() > 2048 {
    return Err("The library title exceeds its ordering limit".into());
  }
  let owner = owner(&item);
  if owner.len() > 256 {
    return Err("The library owner identifier exceeds its limit".into());
  }
  let search = ["title", "artist", "album", "label"]
    .into_iter()
    .filter_map(|key| item[key].as_str())
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
  let encoded = payload(&item)?;
  let hires = hires(&item);
  let genres = serde_json::to_string(&genres(&item)?)
    .map_err(|_| "The library genre metadata could not be encoded")?;
  let (origin_kind, origin_reference) = origin.unwrap_or(("", ""));
  let inserted = connection.prepare_cached("INSERT OR IGNORE INTO entities(generation,kind,reference,canonical_collection,added_key,title_key,search_text,owner,hires,explicit)
    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")
    .map_err(schema::error)?.execute(params![generation.id,kind,reference,collection,-entry.added_at.max(0),title,search,owner,hires,entry.explicit]).map_err(schema::error)? as u64;
  let membership = connection.prepare_cached("INSERT OR IGNORE INTO memberships(generation,kind,reference,collection,added_at,explicit,payload,title_key,search_text,owner,hires,genres,origin_kind,origin_reference)
    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)")
    .map_err(schema::error)?.execute(params![generation.id,kind,reference,collection,entry.added_at.max(0),entry.explicit,encoded,title,search,owner,hires,genres,origin_kind,origin_reference]).map_err(schema::error)?;
  if membership == 0 {
    connection
      .prepare_cached(
        "UPDATE memberships SET
          payload=?7,title_key=?8,search_text=?9,owner=?10,hires=?11,genres=?12
          ,origin_kind=?1,origin_reference=?2
          WHERE generation=?3 AND kind=?4 AND reference=?5 AND collection=?6
          AND (?1='' OR (origin_kind,origin_reference)>=(?1,?2))",
      )
      .map_err(schema::error)?
      .execute(params![
        origin_kind,
        origin_reference,
        generation.id,
        kind,
        reference,
        collection,
        encoded,
        title,
        search,
        owner,
        hires,
        genres
      ])
      .map_err(schema::error)?;
    connection
      .prepare_cached(
        "UPDATE memberships SET added_at=MAX(added_at,?1),explicit=MAX(explicit,?2)
      WHERE generation=?3 AND kind=?4 AND reference=?5 AND collection=?6",
      )
      .map_err(schema::error)?
      .execute(params![
        entry.added_at.max(0),
        entry.explicit,
        generation.id,
        kind,
        reference,
        collection
      ])
      .map_err(schema::error)?;
  }
  refresh_entity(connection, generation, kind, reference)?;
  Ok(IngestStats { new_entities: inserted, new_memberships: membership as u64 })
}

pub(super) fn refresh_entity(
  connection: &Connection,
  generation: Generation,
  kind: &str,
  reference: &str,
) -> IndexResult<()> {
  // A complete source payload wins by fixed priority, regardless of refresh order.
  connection.prepare_cached("UPDATE entities SET (canonical_collection,title_key,search_text,owner,hires)=(
      SELECT collection,title_key,search_text,owner,hires FROM memberships
      WHERE generation=?1 AND kind=?2 AND reference=?3
      ORDER BY explicit DESC,(collection LIKE 'derived:%'),CASE collection WHEN 'purchases' THEN 3 WHEN 'derived:purchases' THEN 3 WHEN 'subscribed' THEN 2 WHEN 'followed' THEN 1 ELSE 0 END DESC LIMIT 1),
    added_key=(SELECT -COALESCE(MAX(CASE WHEN explicit=1 THEN added_at END),MAX(added_at)) FROM memberships WHERE generation=?1 AND kind=?2 AND reference=?3),
    explicit=(SELECT MAX(explicit) FROM memberships WHERE generation=?1 AND kind=?2 AND reference=?3)
    WHERE generation=?1 AND kind=?2 AND reference=?3")
    .map_err(schema::error)?.execute(params![generation.id,kind,reference]).map_err(schema::error)?;
  connection
    .prepare_cached("DELETE FROM entity_genres WHERE generation=?1 AND kind=?2 AND reference=?3")
    .map_err(schema::error)?
    .execute(params![generation.id, kind, reference])
    .map_err(schema::error)?;
  connection.prepare_cached("INSERT INTO entity_genres(generation,kind,reference,genre)
    SELECT ?1,?2,?3,value FROM json_each((SELECT m.genres FROM memberships m JOIN entities e
      ON e.generation=m.generation AND e.kind=m.kind AND e.reference=m.reference AND e.canonical_collection=m.collection
      WHERE m.generation=?1 AND m.kind=?2 AND m.reference=?3))")
    .map_err(schema::error)?.execute(params![generation.id,kind,reference]).map_err(schema::error)?;
  Ok(())
}

pub(super) fn refresh_candidates(
  connection: &Connection,
  generation: Generation,
  source: i64,
  after: (&str, &str),
  through: (&str, &str),
) -> IndexResult<()> {
  connection
    .prepare_cached(REFRESH_CANDIDATES_SQL)
    .map_err(schema::error)?
    .execute(params![generation.id, source, after.0, after.1, through.0, through.1])
    .map_err(schema::error)?;
  connection
    .prepare_cached(CLEAR_CANDIDATE_GENRES_SQL)
    .map_err(schema::error)?
    .execute(params![generation.id, source, after.0, after.1, through.0, through.1])
    .map_err(schema::error)?;
  Ok(())
}

pub(super) const REFRESH_CANDIDATES_SQL: &str = "UPDATE entities AS e SET (canonical_collection,title_key,search_text,owner,hires)=(
    SELECT collection,title_key,search_text,owner,hires FROM memberships m
    WHERE m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference
    ORDER BY explicit DESC,(collection LIKE 'derived:%'),CASE collection WHEN 'purchases' THEN 3 WHEN 'derived:purchases' THEN 3 WHEN 'subscribed' THEN 2 WHEN 'followed' THEN 1 ELSE 0 END DESC LIMIT 1),
    added_key=(SELECT -COALESCE(MAX(CASE WHEN explicit=1 THEN added_at END),MAX(added_at)) FROM memberships m
      WHERE m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference),
    explicit=(SELECT MAX(explicit) FROM memberships m WHERE m.generation=e.generation AND m.kind=e.kind AND m.reference=e.reference)
    WHERE e.generation=?1 AND (e.kind,e.reference) IN (SELECT c.kind,c.reference FROM entities c
      WHERE c.generation=?2 AND (c.kind,c.reference)>(?3,?4) AND (c.kind,c.reference)<=(?5,?6))";

pub(super) const CLEAR_CANDIDATE_GENRES_SQL: &str = "DELETE FROM entity_genres WHERE generation=?1 AND (kind,reference) IN (
    SELECT c.kind,c.reference FROM entities c WHERE c.generation=?2 AND (c.kind,c.reference)>(?3,?4) AND (c.kind,c.reference)<=(?5,?6))";
