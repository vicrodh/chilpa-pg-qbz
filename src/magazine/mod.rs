mod cache;
mod content;

use crate::runtime::{RpcFault, RpcResult, SessionContext};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cache::AccountCache;
use qobuz_api::editorial::{valid_slug, valid_story, EditorialPage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc, Mutex};

#[derive(Default)]
pub(crate) struct Magazine {
  account: Mutex<Option<(u64, Arc<AccountCache>)>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
  section: String,
  offset: u32,
}

fn error_text(message: &str) -> String {
  message
    .char_indices()
    .take_while(|(index, character)| index + character.len_utf8() <= 2048)
    .map(|(_, character)| character)
    .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
    .collect()
}

async fn serialized<F: std::future::Future>(
  gate: &tokio::sync::Mutex<()>,
  timeout: std::time::Duration,
  operation: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
  tokio::time::timeout(timeout, async {
    let _gate = gate.lock().await;
    operation.await
  })
  .await
}

impl Listing {
  fn read(reference: &str, request: &Value) -> Result<Self, RpcFault> {
    let mut query = if reference == "magazine:home" {
      Self::default()
    } else if let Some(encoded) = reference.strip_prefix("magazine:v1:") {
      if encoded.len() > 2048 {
        return Err(RpcFault::invalid("magazine reference"));
      }
      let bytes =
        URL_SAFE_NO_PAD.decode(encoded).map_err(|_| RpcFault::invalid("magazine reference"))?;
      serde_json::from_slice(&bytes).map_err(|_| RpcFault::invalid("magazine reference"))?
    } else {
      return Err(RpcFault::invalid("magazine reference"));
    };
    if let Some(section) = request.pointer("/filters/section") {
      query.section = section.as_str().ok_or_else(|| RpcFault::invalid("magazine section"))?.into();
      query.offset = 0;
    }
    if (!query.section.is_empty() && !valid_slug(&query.section)) || query.offset > 10000 {
      return Err(RpcFault::invalid("magazine section"));
    }
    Ok(query)
  }

  fn reference(&self) -> String {
    format!("magazine:v1:{}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap()))
  }
}

impl Magazine {
  pub fn reset(&self) {
    *self.account.lock().unwrap_or_else(|error| error.into_inner()) = None;
  }

  fn account(&self, session: &SessionContext, user: u64) -> Result<Arc<AccountCache>, RpcFault> {
    let mut account = self.account.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((_, cache)) = account.as_ref().filter(|(id, _)| *id == user) {
      return Ok(cache.clone());
    }
    let cache = AccountCache::open(&session.cache, user)?;
    *account = Some((user, cache.clone()));
    Ok(cache)
  }

  pub async fn page(&self, session: &SessionContext, request: Value) -> RpcResult {
    session.require_identity().await?;
    let reference = request["ref"].as_str().unwrap_or_default();
    let story = reference.strip_prefix("magazine:story:");
    let listing = if let Some(id) = story {
      if !valid_story(id) {
        return Err(RpcFault::invalid("magazine article"));
      }
      None
    } else {
      Some(Listing::read(reference, &request)?)
    };
    let user = session.api.session().await.ok_or_else(|| RpcFault::new(-32001, "Sign in first"))?;
    let account = self.account(session, user.user_id)?;
    let language = session.language().unwrap_or_else(|| "en".into());
    let key = format!(
      "v1:{language}:{}",
      listing.as_ref().map(Listing::reference).unwrap_or_else(|| reference.into())
    );
    let now = crate::catalog::now() as i64;
    let online = session.network.load(Ordering::Relaxed);
    let cached = account.get(&key)?;
    let fresh = if story.is_some() { 86400 } else { 300 };
    let refresh = request["refresh"] == true;
    if cached
      .as_ref()
      .is_some_and(|(_, at)| !online || (!refresh && (0..fresh).contains(&(now - at))))
    {
      let (mut page, at) = cached.unwrap();
      page["offline"] = json!(!online);
      page["stale"] = json!(!(0..fresh).contains(&(now - at)));
      return Ok(page);
    }
    if !online {
      return Err(RpcFault::new(
        -32004,
        "This magazine page has not been saved for offline reading",
      ));
    }
    let previous_store = account.store();
    let fetched = serialized(&account.gate, std::time::Duration::from_secs(10), async {
      if !refresh {
        if let Some((page, _)) =
          account.get(&key)?.filter(|(_, at)| (0..fresh).contains(&(now - at)))
        {
          return Ok(page);
        }
      }
      let store = match account.store().filter(|_| !refresh) {
        Some(store) => store,
        None => {
          let store = session.api.editorial_store().await?;
          account.set_store(&store)?;
          store
        }
      };
      let page = if let Some(id) = story {
        content::article(&session.api.editorial_page(&store, EditorialPage::Story(id)).await?, id)?
      } else {
        self
          .listing(&account, session, &store, listing.as_ref().unwrap(), &language, refresh)
          .await?
      };
      account.put(&key, &page, now)?;
      Ok(page)
    })
    .await
    .unwrap_or_else(|_| Err(RpcFault::new(-32000, "Magazine did not respond in time")));
    match fetched {
      Ok(page) => Ok(page),
      Err(error) => {
        if let Some((mut page, _)) = cached.filter(|_| previous_store == account.store()) {
          page["stale"] = true.into();
          page["error"] = error_text(&error.message).into();
          Ok(page)
        } else {
          Err(error)
        }
      }
    }
  }

  async fn listing(
    &self,
    account: &AccountCache,
    session: &SessionContext,
    store: &str,
    query: &Listing,
    language: &str,
    refresh: bool,
  ) -> RpcResult {
    let nav_key = format!("navigation:{language}");
    let now = crate::catalog::now() as i64;
    let cached = account.get(&nav_key)?;
    let options = match cached.filter(|(_, at)| !refresh && (0..86400).contains(&(now - at))) {
      Some((options, _)) => options,
      None => {
        let nav = session.api.editorial_page(store, EditorialPage::Navigation).await?;
        let options = json!(content::sections(&nav, language)?);
        account.put(&nav_key, &options, now)?;
        options
      }
    };
    if !query.section.is_empty()
      && !options.as_array().into_iter().flatten().any(|option| option["value"] == query.section)
    {
      return Err(RpcFault::invalid("magazine section"));
    }
    let (items, more) = if query.section.is_empty() {
      (content::home(&session.api.editorial_page(store, EditorialPage::Home).await?)?, false)
    } else {
      let wire = session
        .api
        .editorial_page(
          store,
          EditorialPage::Section { slug: &query.section, offset: query.offset, limit: 20 },
        )
        .await?;
      let rows =
        wire.as_array().ok_or_else(|| RpcFault::new(-32000, "Invalid magazine listing"))?;
      (
        rows.iter().take(20).filter_map(content::teaser).collect(),
        rows.len() >= 20 && query.offset < 10000,
      )
    };
    let next = more
      .then(|| Listing { section: query.section.clone(), offset: query.offset + 20 }.reference());
    let spanish = language == "es";
    let mut choices = vec![json!({"value":"","label":if spanish {"Destacados"} else {"Featured"}})];
    choices.extend(options.as_array().into_iter().flatten().cloned());
    Ok(json!({"layout":"collection","title":"Magazine","query_ref":"magazine:home",
      "items":items,"next":next,"has_more":more,"filters":{"section":query.section},
      "filter_controls":[{"key":"section","type":"choice","label":if spanish {"Sección"} else {"Section"},"options":choices}],
      "show_tabs":false}))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn listing_references_round_trip_and_new_filters_restart_pagination() {
    let query = Listing { section: "/hi-fi/tests".into(), offset: 40 };
    let encoded = query.reference();
    assert_eq!(Listing::read(&encoded, &json!({})).unwrap().offset, 40);
    let next = Listing::read(&encoded, &json!({"filters":{"section":"/news"}})).unwrap();
    assert_eq!(next.offset, 0);
    assert_eq!(next.section, "/news");
    for reference in ["magazine:web", "magazine:v1:invalid", "magazine:story:../bad"] {
      assert!(Listing::read(reference, &json!({})).is_err());
    }
  }
}

#[cfg(test)]
mod error_tests {
  use super::*;
  #[test]
  fn cached_page_errors_fit_the_native_contract_and_keep_utf8_boundaries() {
    let message = format!("{}\0\u{1}", "é".repeat(2048));
    let text = error_text(&message);
    assert!(text.len() <= 2048);
    assert!(!text.chars().any(char::is_control));
  }
}

#[cfg(test)]
mod deadline_tests {
  use super::*;
  #[tokio::test]
  async fn the_request_deadline_includes_waiting_for_another_article() {
    let gate = tokio::sync::Mutex::new(());
    let held = gate.lock().await;
    let called = std::sync::atomic::AtomicBool::new(false);
    let result = serialized(&gate, std::time::Duration::from_millis(10), async {
      called.store(true, Ordering::Relaxed);
      42
    })
    .await;
    assert!(result.is_err());
    assert!(!called.load(Ordering::Relaxed));
    drop(held);
    assert_eq!(
      serialized(&gate, std::time::Duration::from_millis(100), async { 42 }).await.unwrap(),
      42
    );
  }
}
