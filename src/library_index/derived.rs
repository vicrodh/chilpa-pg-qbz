use super::*;
use rusqlite::params;
use serde_json::json;
use std::collections::BTreeSet;

fn child(
  connection: &Connection,
  generation: Generation,
  collection: &str,
  parent: (&str, &str),
  added_at: i64,
  kind: &str,
  reference: &str,
  title: &str,
  image: Option<&str>,
) -> IndexResult<u64> {
  if !reference.starts_with(&format!("{kind}:"))
    || reference.len() <= kind.len() + 1
    || title.trim().is_empty()
  {
    return Ok(0);
  }
  let mut item = json!({"kind":kind,"ref":reference,"title":title,"inferred":true});
  if let Some(image) = image.filter(|image| !image.is_empty()) {
    item["image"] = image.into();
  }
  Ok(
    super::member::put_member(
      connection,
      generation,
      collection,
      &IndexedItem { item: &item, added_at, explicit: false },
      Some(parent),
    )?
    .new_entities,
  )
}

pub(super) fn from_parent(
  connection: &Connection,
  generation: Generation,
  collection: &str,
  entry: &IndexedItem<'_>,
) -> IndexResult<u64> {
  let kind = entry.item["kind"].as_str().unwrap_or_default();
  if !matches!(kind, "album" | "track") || !matches!(collection, "favorites" | "purchases") {
    return Ok(0);
  }
  let reference =
    entry.item["ref"].as_str().ok_or("The inferred entity has no parent reference")?;
  let parent = (kind, reference);
  let collection = format!("derived:{collection}");
  let mut inserted = 0;
  let mut artists = BTreeSet::new();
  if let Some(values) = entry.item.get("library_artists").filter(|value| !value.is_null()) {
    let values = values.as_array().ok_or("The library artist relationships must be an array")?;
    if values.len() > 32 {
      return Err("The library artist relationship limit was reached".into());
    }
    for value in values {
      let reference = value["ref"].as_str().unwrap_or_default();
      if !artists.insert(reference.to_string()) {
        continue;
      }
      inserted += child(
        connection,
        generation,
        &collection,
        parent,
        entry.added_at,
        "artist",
        reference,
        value["title"].as_str().unwrap_or_default(),
        value["image"].as_str(),
      )?;
    }
  }
  if let Some(reference) =
    entry.item["artist_ref"].as_str().filter(|reference| !artists.contains(*reference))
  {
    if artists.len() >= 32 {
      return Err("The library artist relationship limit was reached".into());
    }
    inserted += child(
      connection,
      generation,
      &collection,
      parent,
      entry.added_at,
      "artist",
      reference,
      entry.item["artist"].as_str().unwrap_or_default(),
      entry.item["artist_image"].as_str(),
    )?;
  }
  if let Some(reference) = entry.item["label_ref"].as_str() {
    inserted += child(
      connection,
      generation,
      &collection,
      parent,
      entry.added_at,
      "label",
      reference,
      entry.item["label"].as_str().unwrap_or_default(),
      entry.item["label_image"].as_str(),
    )?;
  }
  Ok(inserted)
}

pub(super) const PARENTS_SQL: &str = "SELECT p.collection,p.payload,p.added_at,p.explicit FROM memberships p
    WHERE p.generation=?1 AND p.kind IN ('album','track') AND p.collection IN ('favorites','purchases')
      AND (p.kind,p.reference) IN (SELECT c.kind,c.reference FROM entities c
        WHERE c.generation=?2 AND (c.kind,c.reference)>(?3,?4) AND (c.kind,c.reference)<=(?5,?6))
    ORDER BY p.kind,p.reference,p.collection";

pub(super) fn rebuild_parents(
  connection: &Connection,
  generation: Generation,
  source: i64,
  after: (&str, &str),
  through: (&str, &str),
) -> IndexResult<u64> {
  let mut statement = connection.prepare_cached(PARENTS_SQL).map_err(schema::error)?;
  let mut rows = statement
    .query(params![generation.id, source, after.0, after.1, through.0, through.1])
    .map_err(schema::error)?;
  let mut inserted = 0;
  while let Some(row) = rows.next().map_err(schema::error)? {
    let collection: String = row.get(0).map_err(schema::error)?;
    let payload: String = row.get(1).map_err(schema::error)?;
    let item: Value = serde_json::from_str(&payload)
      .map_err(|_| "The indexed parent relationship is unreadable")?;
    inserted += from_parent(
      connection,
      generation,
      &collection,
      &IndexedItem {
        item: &item,
        added_at: row.get(2).map_err(schema::error)?,
        explicit: row.get(3).map_err(schema::error)?,
      },
    )?;
  }
  Ok(inserted)
}
