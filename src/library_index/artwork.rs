use super::*;
use rusqlite::params;
use std::collections::BTreeMap;

const CACHE_ENTRIES: usize = 512;
const MAX_URL_BYTES: usize = 4096;
const FRESH_SECONDS: i64 = 86400;

#[derive(Debug)]
struct Artwork {
  image: Option<String>,
  retry_at: i64,
}

fn image(value: &Value) -> Option<String> {
  let image = crate::projection::image(value)?;
  let url = url::Url::parse(&image).ok()?;
  (image.len() <= MAX_URL_BYTES
    && url.scheme() == "https"
    && url.host_str() == Some("static.qobuz.com")
    && url.username().is_empty()
    && url.password().is_none()
    && url.port_or_known_default() == Some(443))
  .then_some(image)
}

impl LibraryIndex {
  fn artwork(&self, ids: &[u64]) -> IndexResult<BTreeMap<u64, Artwork>> {
    if ids.len() > MAX_PAGE {
      return Err("The artwork page exceeds 100 labels".into());
    }
    let connection = self.read();
    let mut statement = connection
      .prepare_cached("SELECT image,retry_at FROM artwork WHERE reference=?1")
      .map_err(schema::error)?;
    let mut records = BTreeMap::new();
    for id in ids {
      if let Some(record) = statement
        .query_row([format!("label:{id}")], |row| {
          Ok(Artwork { image: row.get(0)?, retry_at: row.get(1)? })
        })
        .optional()
        .map_err(schema::error)?
      {
        records.insert(*id, record);
      }
    }
    Ok(records)
  }

  fn remember_artwork(
    &self,
    ids: &[u64],
    records: Option<&BTreeMap<u64, String>>,
    now: i64,
  ) -> IndexResult<()> {
    if ids.len() > MAX_PAGE {
      return Err("The artwork page exceeds 100 labels".into());
    }
    let mut connection = self.lock();
    let transaction = connection.transaction().map_err(schema::error)?;
    for id in ids {
      let image = records.and_then(|records| records.get(id));
      if image.is_some_and(|image| image.len() > MAX_URL_BYTES) {
        return Err("The label artwork URL is too long".into());
      }
      let lifetime = if records.is_none() {
        60
      } else if image.is_none() {
        600
      } else {
        FRESH_SECONDS
      };
      transaction.execute(
        "INSERT INTO artwork(reference,image,checked_at,retry_at) VALUES(?1,?2,?3,?4)
        ON CONFLICT(reference) DO UPDATE SET
        image=CASE WHEN ?5 THEN excluded.image ELSE artwork.image END,checked_at=excluded.checked_at,retry_at=excluded.retry_at",
        params![format!("label:{id}"), image, now, now.saturating_add(lifetime), records.is_some()],
      ).map_err(schema::error)?;
    }
    transaction
      .execute(
        "DELETE FROM artwork WHERE reference IN
      (SELECT reference FROM artwork ORDER BY checked_at DESC,reference LIMIT -1 OFFSET ?1)",
        [CACHE_ENTRIES as i64],
      )
      .map_err(schema::error)?;
    transaction.commit().map_err(schema::error)
  }
}

fn identifiers(items: &[Value]) -> Vec<u64> {
  items
    .iter()
    .filter(|item| item["kind"] == "label" && image(item).is_none())
    .filter_map(|item| item["ref"].as_str()?.strip_prefix("label:")?.parse().ok())
    .filter(|id| *id > 0)
    .take(MAX_PAGE)
    .collect()
}

fn apply(items: &mut [Value], records: &BTreeMap<u64, Artwork>, overwrite: bool) {
  for item in items.iter_mut().filter(|item| item["kind"] == "label") {
    item["art_fit"] = "contain".into();
    if !overwrite && image(item).is_some() {
      continue;
    }
    let id = item["ref"]
      .as_str()
      .and_then(|reference| reference.strip_prefix("label:"))
      .and_then(|id| id.parse::<u64>().ok());
    if let Some(record) = id.and_then(|id| records.get(&id)) {
      if let Some(image) = &record.image {
        item["image"] = image.clone().into();
      } else if overwrite {
        if let Some(fields) = item.as_object_mut() {
          let _ = fields.remove("image");
        }
      }
    }
  }
}

/// One visible page, one request at most; 512 persisted URLs per account.
/// Offline and failures keep previous artwork. No worker runs between reads.
pub(crate) async fn enrich(
  index: &LibraryIndex,
  api: &qobuz_api::QobuzClient,
  items: &mut [Value],
  online: bool,
  refresh: bool,
) -> IndexResult<()> {
  let ids = identifiers(items);
  if ids.is_empty() {
    return Ok(());
  }
  let mut records = index.artwork(&ids)?;
  apply(items, &records, false);
  if !online {
    return Ok(());
  }
  let Ok(_reservation) = index.artwork_guard.try_lock() else {
    return Ok(());
  };
  let now = crate::catalog::now() as i64;
  let missing: Vec<_> = ids
    .into_iter()
    .filter(|id| refresh || records.get(id).is_none_or(|record| record.retry_at <= now))
    .collect();
  if missing.is_empty() {
    return Ok(());
  }
  let found =
    match tokio::time::timeout(std::time::Duration::from_secs(3), api.get_label_list(&missing))
      .await
    {
      Ok(Ok(page)) => Some(
        page
          .labels
          .into_iter()
          .filter(|label| missing.contains(&label.id))
          .filter_map(|label| {
            let card = serde_json::to_value(&label).ok()?;
            Some((label.id, image(&card)?))
          })
          .collect(),
      ),
      _ => None,
    };
  index.remember_artwork(&missing, found.as_ref(), now)?;
  records = index.artwork(&missing)?;
  apply(items, &records, true);
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn artwork_survives_restart_and_failure_without_changing_membership() {
    let root = tempfile::tempdir().unwrap();
    let url = "https://static.qobuz.com/images/labels/example.jpg".to_string();
    {
      let index = LibraryIndex::open(root.path(), 1).unwrap();
      index.remember_artwork(&[2], Some(&BTreeMap::from([(2, url.clone())])), 10).unwrap();
      index.remember_artwork(&[2], None, 20).unwrap();
      assert_eq!(index.status().unwrap().items, 0);
    }
    let index = LibraryIndex::open(root.path(), 1).unwrap();
    let records = index.artwork(&[2]).unwrap();
    assert_eq!(records[&2].image.as_ref(), Some(&url));
    assert_eq!(records[&2].retry_at, 80);
    let mut items = vec![json!({"kind":"label","ref":"label:2","followed":false,"inferred":true})];
    crate::projection::image_fallback(&mut items[0]);
    assert_eq!(identifiers(&items), vec![2]);
    apply(&mut items, &records, false);
    crate::projection::image_fallback(&mut items[0]);
    assert_eq!(items[0]["image"], url);
    assert_eq!(items[0]["image_fallback"], "disc");
    assert!(identifiers(&items).is_empty());
    assert_eq!(items[0]["followed"], false);
    assert_eq!(items[0]["inferred"], true);
  }

  #[test]
  fn cache_and_negative_results_have_limits() {
    let root = tempfile::tempdir().unwrap();
    let index = LibraryIndex::open(root.path(), 1).unwrap();
    for batch in 0..6 {
      index
        .remember_artwork(
          &(1..=100).map(|id| id + batch * 100).collect::<Vec<_>>(),
          Some(&BTreeMap::new()),
          batch as i64,
        )
        .unwrap();
    }
    let count: i64 =
      index.read().query_row("SELECT COUNT(*) FROM artwork", [], |row| row.get(0)).unwrap();
    assert_eq!(count, CACHE_ENTRIES as i64);
    assert_eq!(index.artwork(&[600]).unwrap()[&600].retry_at, 605);
    assert!(index.artwork(&vec![1; 101]).is_err());
    assert!(index
      .remember_artwork(&[1], Some(&BTreeMap::from([(1, "x".repeat(MAX_URL_BYTES + 1))])), 0)
      .is_err());
  }

  #[test]
  fn only_public_label_artwork_is_accepted() {
    for url in ["https://static.qobuz.com/a.png", "https://static.qobuz.com/a.png?size=300"] {
      assert_eq!(image(&json!({"image":url})).as_deref(), Some(url));
    }
    for url in ["file:///tmp/a", "https://secret@static.qobuz.com/a", "https://other.test/a"] {
      assert!(image(&json!({"image":url})).is_none());
    }
  }
}

#[cfg(test)]
mod refresh_tests {
  use super::*;
  use serde_json::json;

  #[tokio::test]
  async fn explicit_refresh_bypasses_freshness_and_a_failed_fetch_keeps_the_saved_logo() {
    let root = tempfile::tempdir().unwrap();
    let index = LibraryIndex::open(root.path(), 1).unwrap();
    let now = crate::catalog::now() as i64;
    let url = "https://static.qobuz.com/images/labels/saved.jpg".to_string();
    index.remember_artwork(&[1], Some(&BTreeMap::from([(1, url.clone())])), now).unwrap();
    let api = qobuz_api::QobuzClient::new().unwrap();
    let raw = json!({"kind":"label","ref":"label:1"});
    let mut items = vec![raw.clone()];
    enrich(&index, &api, &mut items, true, false).await.unwrap();
    assert_eq!(index.artwork(&[1]).unwrap()[&1].retry_at, now + FRESH_SECONDS);
    let mut items = vec![raw];
    // No credentials: the forced request fails before making a network call.
    enrich(&index, &api, &mut items, true, true).await.unwrap();
    let record = index.artwork(&[1]).unwrap();
    assert!(record[&1].retry_at < now + 180);
    assert_eq!(items[0]["image"], url);
  }

  #[test]
  fn a_successful_empty_result_removes_a_retired_logo() {
    let root = tempfile::tempdir().unwrap();
    let index = LibraryIndex::open(root.path(), 1).unwrap();
    let url = "https://static.qobuz.com/images/labels/old.jpg".to_string();
    index.remember_artwork(&[1], Some(&BTreeMap::from([(1, url.clone())])), 1).unwrap();
    index.remember_artwork(&[1], Some(&BTreeMap::new()), 2).unwrap();
    let mut items = vec![json!({"kind":"label","ref":"label:1","image":url,"followed":false})];
    apply(&mut items, &index.artwork(&[1]).unwrap(), true);
    assert!(items[0].get("image").is_none());
    assert_eq!(items[0]["followed"], false);
  }
}
