use super::*;
use rusqlite::params;
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

// One account's catalog shares the index connection; reads create only bounded temporaries.
const MAX_AWARDS: usize = 2048;
const MAX_BYTES: usize = 1024 * 1024;
const FRESH_SECONDS: i64 = 86400;
const STALE_CURSOR: &str = "The library cursor is stale; reload the first page";

#[derive(Default)]
struct Stamp {
  available: bool,
  checked_at: i64,
  retry_at: i64,
  revision: i64,
}

pub(crate) struct AwardPage {
  pub page: Page,
  pub total: usize,
  pub catalog_revision: i64,
  pub stale: bool,
  pub error: Option<String>,
}

impl LibraryIndex {
  fn award_stamp(&self) -> IndexResult<Stamp> {
    self
      .read()
      .query_row(
        "SELECT payload IS NOT NULL,checked_at,retry_at,revision FROM award_catalog WHERE id=1",
        [],
        |row| {
          Ok(Stamp {
            available: row.get(0)?,
            checked_at: row.get(1)?,
            retry_at: row.get(2)?,
            revision: row.get(3)?,
          })
        },
      )
      .optional()
      .map_err(schema::error)
      .map(Option::unwrap_or_default)
  }

  fn remember_awards(&self, items: &[Value], now: i64) -> IndexResult<()> {
    let payload =
      serde_json::to_string(items).map_err(|_| "The award catalog cannot be encoded")?;
    if items.len() > MAX_AWARDS || payload.len() > MAX_BYTES {
      return Err("The award catalog exceeds 2048 items or 1 MiB".into());
    }
    self.lock().execute("INSERT INTO award_catalog(id,payload,checked_at,retry_at,revision) VALUES(1,?1,?2,?2,1)
      ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,checked_at=excluded.checked_at,retry_at=excluded.retry_at,
      revision=award_catalog.revision+(award_catalog.payload IS NOT excluded.payload)",params![payload,now]).map_err(schema::error)?;
    Ok(())
  }

  fn defer_awards(&self, now: i64) -> IndexResult<()> {
    self
      .lock()
      .execute(
        "INSERT INTO award_catalog(id,payload,checked_at,retry_at,revision) VALUES(1,NULL,0,?1,0)
      ON CONFLICT(id) DO UPDATE SET retry_at=excluded.retry_at",
        [now.saturating_add(60)],
      )
      .map_err(schema::error)?;
    Ok(())
  }

  fn award_snapshot(&self) -> IndexResult<(IndexStatus, Option<Vec<Value>>, Vec<Value>, Stamp)> {
    let mut reader = self.read();
    let snapshot = reader.transaction().map_err(schema::error)?;
    let status = schema::status(&snapshot, self.account)?;
    let cached = snapshot
      .query_row(
        "SELECT payload,checked_at,retry_at,revision FROM award_catalog WHERE id=1",
        [],
        |row| {
          let payload: Option<String> = row.get(0)?;
          let stamp = Stamp {
            available: payload.is_some(),
            checked_at: row.get(1)?,
            retry_at: row.get(2)?,
            revision: row.get(3)?,
          };
          Ok((payload, stamp))
        },
      )
      .optional()
      .map_err(schema::error)?;
    let (payload, stamp) = cached.unwrap_or_default();
    let catalog = payload
      .map(|payload| -> IndexResult<Vec<Value>> {
        if payload.len() > MAX_BYTES {
          return Err("The cached award catalog exceeds 1 MiB".into());
        }
        let items: Vec<Value> =
          serde_json::from_str(&payload).map_err(|_| "The cached award catalog is unreadable")?;
        if items.len() > MAX_AWARDS {
          return Err("The cached award catalog exceeds 2048 items".into());
        }
        Ok(items)
      })
      .transpose()?;
    let mut statement=snapshot.prepare("SELECT payload,added_at FROM memberships WHERE generation=?1 AND kind='award' AND collection='followed' ORDER BY reference LIMIT ?2").map_err(schema::error)?;
    let mut rows = statement
      .query(params![status.generation, (MAX_AWARDS + 1) as i64])
      .map_err(schema::error)?;
    let mut followed = Vec::new();
    let mut bytes = 0_usize;
    while let Some(row) = rows.next().map_err(schema::error)? {
      let payload: String = row.get(0).map_err(schema::error)?;
      bytes = bytes.saturating_add(payload.len());
      if bytes > MAX_BYTES || followed.len() >= MAX_AWARDS {
        return Err("Followed awards exceed 2048 items or 1 MiB".into());
      }
      let mut card: Value =
        serde_json::from_str(&payload).map_err(|_| "A followed award is unreadable")?;
      card["added_at"] = json!(row.get::<_, i64>(1).map_err(schema::error)?);
      followed.push(card);
    }
    Ok((status, catalog, followed, stamp))
  }

  pub async fn awards_page(
    &self,
    api: &qobuz_api::QobuzClient,
    query: &Query,
    cursor: Option<&str>,
    limit: usize,
    online: bool,
    refresh: bool,
  ) -> IndexResult<AwardPage> {
    let now = crate::catalog::now() as i64;
    let mut error = None;
    let stamp = self.award_stamp()?;
    if online
      && (refresh
        || (!stamp.available || !(0..FRESH_SECONDS).contains(&(now - stamp.checked_at)))
          && now >= stamp.retry_at)
    {
      let result = tokio::time::timeout(Duration::from_secs(10), async {
        let _guard = self.awards_guard.lock().await;
        let stamp = self.award_stamp()?;
        if !refresh && stamp.available && (0..FRESH_SECONDS).contains(&(now - stamp.checked_at)) {
          return Ok(());
        }
        if !refresh && now < stamp.retry_at {
          return Ok(());
        }
        let items = fetch(api).await?;
        self.remember_awards(&items, now)
      })
      .await
      .unwrap_or_else(|_| Err("The award catalog request timed out".into()));
      if let Err(message) = result {
        self.defer_awards(now)?;
        error = Some(message)
      }
    }
    let (status, catalog, followed, stamp) = self.award_snapshot()?;
    let following = only_followed(query);
    if catalog.is_none() && !following {
      return Err(
        error
          .unwrap_or_else(|| "The award catalog is unavailable; connect and refresh Awards".into()),
      );
    }
    let stale = !stamp.available || !(0..FRESH_SECONDS).contains(&(now - stamp.checked_at));
    let page = select(status, catalog.unwrap_or_default(), followed, query, cursor, limit)?;
    Ok(AwardPage { page: page.0, total: page.1, catalog_revision: stamp.revision, stale, error })
  }
}

fn only_followed(query: &Query) -> bool {
  query
    .membership_groups
    .iter()
    .flatten()
    .any(|m| matches!(m, Membership::Favorites | Membership::Followed))
}

async fn fetch(api: &qobuz_api::QobuzClient) -> IndexResult<Vec<Value>> {
  let mut cards = BTreeMap::new();
  let mut offset = 0_u32;
  let mut bytes = 2;
  loop {
    let response = api
      .get_award_explore(100, offset)
      .await
      .map_err(|_| "The award catalog could not be refreshed")?;
    let items = response["items"]
      .as_array()
      .filter(|items| items.len() <= 100)
      .ok_or("The award catalog page is invalid")?;
    let more = response["has_more"].as_bool().ok_or("The award catalog has no pagination state")?;
    if more && items.is_empty() {
      return Err("The award catalog did not advance".into());
    }
    for item in items {
      let card =
        crate::library_sources::thin_card(item, "award", 0).map_err(|error| error.to_string())?;
      let reference = card["ref"].as_str().ok_or("The award has no identity")?.to_owned();
      bytes += serde_json::to_vec(&card).map_err(|_| "The award cannot be encoded")?.len() + 1;
      if cards.len() >= MAX_AWARDS || bytes > MAX_BYTES {
        return Err("The award catalog exceeds 2048 items or 1 MiB".into());
      }
      if cards.insert(reference, card).is_some() {
        return Err("The award catalog repeated an identity".into());
      }
    }
    if !more {
      break;
    }
    offset = offset.checked_add(items.len() as u32).ok_or("The award catalog offset overflowed")?;
  }
  Ok(cards.into_values().collect())
}

fn select(
  status: IndexStatus,
  catalog: Vec<Value>,
  followed: Vec<Value>,
  query: &Query,
  cursor: Option<&str>,
  limit: usize,
) -> IndexResult<(Page, usize)> {
  let mut cards: BTreeMap<String, Value> = BTreeMap::new();
  for mut card in catalog {
    card["favorite"] = false.into();
    card["followed"] = false.into();
    card["in_library"] = false.into();
    card["added_at"] = 0.into();
    card["art_fit"] = "contain".into();
    let reference = card["ref"].as_str().ok_or("The cached award has no identity")?.to_owned();
    cards.insert(reference, card);
  }
  for mut saved in followed {
    let reference = saved["ref"].as_str().ok_or("The followed award has no identity")?.to_owned();
    if let Some(card) = cards.get_mut(&reference) {
      card["favorite"] = true.into();
      card["followed"] = true.into();
      card["in_library"] = true.into();
      card["added_at"] = saved["added_at"].take();
    } else {
      saved["favorite"] = true.into();
      saved["followed"] = true.into();
      saved["in_library"] = true.into();
      saved["art_fit"] = "contain".into();
      cards.insert(reference, saved);
    }
  }
  if cards.len() > MAX_AWARDS {
    return Err("The combined award catalog exceeds 2048 items".into());
  }
  let words: Vec<_> = query.text.split_whitespace().take(17).map(str::to_lowercase).collect();
  if words.len() > 16 {
    return Err("The award search exceeds sixteen words".into());
  }
  let mut cards: Vec<Value> = cards
    .into_values()
    .filter(|card| {
      (!only_followed(query) || card["followed"] == true)
        && words.iter().all(|word| {
          format!(
            "{} {}",
            card["title"].as_str().unwrap_or_default(),
            card["subtitle"].as_str().unwrap_or_default()
          )
          .to_lowercase()
          .contains(word)
        })
    })
    .collect();
  cards.sort_by_cached_key(|card| {
    let added =
      if query.order == Order::Added { -card["added_at"].as_i64().unwrap_or(0).max(0) } else { 0 };
    (
      added,
      card["title"].as_str().unwrap_or_default().to_lowercase(),
      card["ref"].as_str().unwrap_or_default().to_owned(),
    )
  });
  let encoded = serde_json::to_vec(&(status.account, &query, &cards))
    .map_err(|_| "The award query cannot be encoded")?;
  if encoded.len() > MAX_BYTES * 2 {
    return Err("The combined award query exceeds 2 MiB".into());
  }
  let fingerprint = encoded.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
    (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
  });
  let prefix = format!("awards1:{fingerprint:016x}:");
  let offset = match cursor {
    None => 0,
    Some(cursor) => cursor
      .strip_prefix(&prefix)
      .and_then(|offset| offset.parse::<usize>().ok())
      .filter(|offset| *offset <= cards.len())
      .ok_or(STALE_CURSOR)?,
  };
  let total = cards.len();
  let limit = limit.clamp(1, MAX_PAGE);
  let end = offset.saturating_add(limit).min(total);
  let items = cards.drain(offset..end).collect();
  let next = (end < total).then(|| format!("{prefix}{end}"));
  Ok((Page { items, has_more: next.is_some(), next, status }, total))
}

#[cfg(test)]
#[path = "awards_tests.rs"]
mod tests;
