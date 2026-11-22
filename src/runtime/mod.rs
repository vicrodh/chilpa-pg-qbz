mod fault;
pub mod process;
mod session;

use crate::catalog::{self, CatalogService};
pub use fault::{RpcFault, RpcResult};
use serde_json::{json, Value};
pub(crate) use session::SessionContext;
use std::{
  path::PathBuf,
  sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
  },
  time::Duration,
};
use tokio::sync::RwLock;

pub struct Runtime {
  session: Arc<SessionContext>,
  catalog: CatalogService,
  account_epoch: RwLock<()>,
  playback_negotiated: AtomicBool,
  collections_negotiated: AtomicBool,
  articles_negotiated: AtomicBool,
}

impl Runtime {
  pub fn at(directory: PathBuf) -> Arc<Self> {
    Self::with_directories(directory.clone(), directory)
  }

  pub fn with_directories(directory: PathBuf, cache: PathBuf) -> Arc<Self> {
    let session = Arc::new(SessionContext::new(directory, cache));
    Arc::new(Self {
      catalog: CatalogService::new(session.clone()),
      session,
      account_epoch: RwLock::new(()),
      playback_negotiated: AtomicBool::new(false),
      collections_negotiated: AtomicBool::new(false),
      articles_negotiated: AtomicBool::new(false),
    })
  }

  pub async fn flush_reports(&self) {
    if !self.session.network.load(Ordering::Relaxed) {
      return;
    }
    let _epoch = self.account_epoch.read().await;
    if self.session.load().await.is_ok() {
      self.session.journal.flush(&self.session.api).await;
    }
  }

  pub async fn call(self: &Arc<Self>, method: &str, request: Value) -> RpcResult {
    match method {
      "auth.status" | "auth.login" | "auth.logout" | "settings.open" | "settings.submit" => {
        let _epoch = self.account_epoch.write().await;
        self.account_request(method, request).await
      }
      _ => {
        let _epoch = self.account_epoch.read().await;
        self.read_request(method, request).await
      }
    }
  }

  async fn account_request(&self, method: &str, request: Value) -> RpcResult {
    if !self.session.network.load(Ordering::Relaxed)
      && method == "settings.submit"
      && request["action"] != "sign_out"
    {
      return Err(RpcFault::new(-32000, "Qobuz needs network permission to connect Firefox"));
    }
    if self.session.network.load(Ordering::Relaxed) {
      self.session.load().await?;
    }
    let context = &self.session;
    let language = context.language().unwrap_or_else(|| "en".into());
    if matches!(method, "settings.submit" | "auth.logout") {
      self.catalog.invalidate_catalog();
    }
    match method {
      "auth.status" => Ok(context.access.inspect(&context.api).await),
      "auth.login" | "settings.open" => Ok(context.access.form(&context.api, &language).await),
      "auth.logout" => {
        let account = context.access.sign_out(&context.api).await;
        self.catalog.library.reset();
        let reports = context.journal.reset().await;
        account.and(reports).map_err(|error| RpcFault::new(-32000, error))?;
        Ok(json!({"signed_in":false}))
      }
      "settings.submit" => {
        let page = context.access.apply(&context.api, &language, request).await;
        if !context.api.is_logged_in().await {
          self.catalog.library.reset();
          context.journal.reset().await.map_err(|error| RpcFault::new(-32000, error))?;
        }
        Ok(page)
      }
      _ => unreachable!("account route"),
    }
  }

  async fn read_request(self: &Arc<Self>, method: &str, request: Value) -> RpcResult {
    if matches!(method, "page.get" | "search" | "album.get" | "artist.get" | "playlist.get") {
      let session = self.session.clone();
      tokio::spawn(async move {
        if session.network.load(Ordering::Relaxed) && session.api.is_logged_in().await {
          session.access.warm().await;
        }
      });
    }
    let catalog = &self.catalog;
    match method {
      "initialize" => self.greet(request),
      "playback.event" => self.playback_event(request).await,
      "reporting.status" => {
        self.session.load().await?;
        let mut status = self.session.journal.inspect().await;
        status["host_supports_events"] = json!(self.playback_negotiated.load(Ordering::Relaxed));
        Ok(status)
      }
      "page.action" => catalog.dispatch_action(request).await,
      "page.get" => {
        let reference = request["ref"].as_str().unwrap_or_default();
        if reference == "genre-discovery" {
          return catalog.genre_discovery(&request).await;
        }
        if reference.starts_with("library:") {
          return catalog
            .library
            .page_with_contract(
              &self.session,
              request,
              self.collections_negotiated.load(Ordering::Relaxed),
            )
            .await;
        }
        if reference.starts_with("magazine:") {
          if !self.articles_negotiated.load(Ordering::Relaxed) {
            return Err(RpcFault::invalid("page reference"));
          }
          return catalog.magazine.page(&self.session, request).await;
        }
        catalog.load_page(request).await
      }
      "home" => catalog.memoized("home", Duration::from_secs(300), || catalog.load_home()).await,
      "library.list" => catalog.list_library(request).await,
      "album.get" => catalog.load_album(request).await,
      "artist.get" => catalog.load_artist(request).await,
      "playlist.get" => catalog.load_playlist(request).await,
      "search" => catalog.search_catalog(request).await,
      "track.resolve" => catalog.resolve_track(request).await,
      "track.get" => {
        let identifier = catalog::parse_reference(&request, "track")?
          .parse()
          .map_err(|_| RpcFault::invalid("track"))?;
        Ok(json!({"track":catalog.lookup_track(identifier).await?}))
      }
      "track.lyrics" => catalog.load_lyrics(request).await,
      "favorites.set" => catalog.set_favorite(request).await,
      "link.resolve" => Ok(catalog::classify_link(request["url"].as_str().unwrap_or_default())),
      "inspect" => Ok(json!({"settings_pages":["account"]})),
      _ => Err(RpcFault::new(-32601, format!("no method {method}"))),
    }
  }

  fn greet(self: &Arc<Self>, request: Value) -> RpcResult {
    self.session.access.configure(request["desktop_companion"].clone());
    self.session.network.store(
      request.pointer("/network/enabled").and_then(Value::as_bool).unwrap_or(true),
      Ordering::Relaxed,
    );
    self
      .playback_negotiated
      .store(request["playback_events"].as_u64() == Some(1), Ordering::Relaxed);
    if let Some(locale) = request["locale"].as_str() {
      self.session.set_locale(locale);
    }
    let collection = request["features"]["collection_pages"].as_u64() == Some(1);
    let articles = request["features"]["article_pages"].as_u64() == Some(1);
    self.collections_negotiated.store(collection, Ordering::Relaxed);
    self.articles_negotiated.store(articles, Ordering::Relaxed);
    let session = self.session.clone();
    tokio::spawn(async move {
      if !session.network.load(Ordering::Relaxed) {
        return;
      }
      let _ = session.load().await;
      if let Some(language) = session.language() {
        session.api.set_locale(language).await;
      }
    });
    let mut reply = json!({"protocol":1,"capabilities":{"source_name":"Qobuz","auth":"browser","home":true,
      "library":["albums","tracks","artists","playlists","labels"],"pages":true,"genre_discovery":1,
      "playlist_edit":true,"search":true,"scene_search_limit":1000,"resolve":true,
      "favorites":true,"track_lyrics":1,"playback_events":1}});
    let spanish = self.session.language().as_deref() == Some("es");
    if request["features"]["source_navigation"].as_u64() == Some(1) {
      reply["capabilities"]["source_navigation"] =
        json!(source_navigation(spanish, collection, articles));
    }
    if articles {
      reply["capabilities"]["article_pages"] = json!(1);
    }
    Ok(reply)
  }

  async fn playback_event(&self, request: Value) -> RpcResult {
    self.session.load().await?;
    let Some(identity) = self.session.api.session().await else {
      return Ok(json!({"accepted":false,"discard":true}));
    };
    let interrupted = request["kind"] == "ended"
      && matches!(request["reason"].as_str(), Some("skipped" | "stopped" | "shutdown"));
    let resource = if interrupted {
      self
        .session
        .journal
        .active_resource(request["playback_id"].as_str().unwrap_or_default())
        .await
    } else {
      None
    };
    if let Err(error) = self.session.journal.record(identity.user_id, &request).await {
      match error.as_str() {
        "Unknown stream resolution"
        | "Playback has not started"
        | "Stream resolution belongs to another account"
        | "Playback belongs to another account" => {
          return Ok(json!({"accepted":false,"discard":true}))
        }
        _ => return Err(RpcFault::new(-32000, error)),
      }
    }
    if let Some(resource) = resource {
      self.session.access.stop(&resource).await;
    }
    if request["kind"] == "started" {
      for prefix in ["personal", "page discover:home", "page discover:foryou"] {
        self.catalog.memo.remove_prefix(prefix);
      }
    }
    Ok(json!({"accepted":true}))
  }
}

fn source_navigation(spanish: bool, collections: bool, articles: bool) -> Vec<Value> {
  let mut entries = Vec::new();
  if collections {
    let icons = ["stack2", "playlist", "music", "disc", "user", "tag", "star"];
    let children: Vec<_> = crate::library_sources::LIBRARY_VIEWS
      .into_iter()
      .zip(icons)
      .map(|((id, en, es), icon)| {
        json!({"id":id,"title":if spanish {es} else {en},"icon":icon,
        "ref":format!("library:{id}"),"layout":"collection"})
      })
      .collect();
    entries.push(json!({"id":"library","title":if spanish {"Biblioteca"} else {"Library"},
      "icon":"columns3","ref":"library:all","layout":"collection","children":children}));
  }
  if collections && articles {
    entries.push(json!({"id":"magazine","title":"Magazine","icon":"books",
      "ref":"magazine:home","layout":"collection"}));
  }
  entries
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn magazine_requires_native_articles_and_never_negotiates_web() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::at(directory.path().into());
    for features in [json!({}), json!({"web_pages":1}), json!({"article_pages":true})] {
      let reply = runtime.greet(json!({"features":features})).unwrap();
      assert!(reply["capabilities"].get("article_pages").is_none());
      assert!(reply["capabilities"].get("web_pages").is_none());
      assert!(reply["capabilities"].get("web_origins").is_none());
      assert!(runtime.read_request("page.get", json!({"ref":"magazine:home"})).await.is_err());
    }
    let reply = runtime
      .greet(json!({"locale":"es-MX","features":{
        "source_navigation":1,"collection_pages":1,"article_pages":1,"web_pages":1
      }}))
      .unwrap();
    let entries = reply["capabilities"]["source_navigation"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["children"].as_array().unwrap().len(), 7);
    assert_eq!(entries[1]["ref"], "magazine:home");
    assert_eq!(entries[1]["layout"], "collection");
    assert_eq!(reply["capabilities"]["article_pages"], 1);
    assert!(reply["capabilities"].get("web_pages").is_none());
    assert!(reply["capabilities"]["home"].as_bool().unwrap());
    let old =
      runtime.greet(json!({"features":{"source_navigation":1,"collection_pages":1}})).unwrap();
    assert_eq!(old["capabilities"]["source_navigation"].as_array().unwrap().len(), 1);
    assert!(!source_navigation(false, true, true).iter().any(|entry| entry["id"] == "home"));
  }
}
