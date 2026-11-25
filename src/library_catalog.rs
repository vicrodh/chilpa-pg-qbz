use crate::{
  library_index::{Generation, IndexedItem, LibraryIndex, Membership, Order, Query},
  library_sources::{LibraryGenres, LibrarySource},
  runtime::{RpcFault, RpcResult, SessionContext},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
  sync::{
    atomic::{AtomicU64, Ordering as AtomicOrdering},
    Arc, Mutex,
  },
  time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedRwLockWriteGuard, RwLock};

trait LibraryRemote: Send + std::marker::Sync {
  fn revisions(
    &self,
  ) -> impl std::future::Future<
    Output = Result<qobuz_api::library_pages::LibraryRevisions, qobuz_api::ApiError>,
  > + Send;
  fn source_page(
    &self,
    source: LibrarySource,
    account: u64,
    offset: u64,
    limit: u32,
  ) -> impl std::future::Future<Output = Result<crate::library_sources::SourcePage, qobuz_api::ApiError>>
       + Send;
}

impl LibraryRemote for qobuz_api::QobuzClient {
  fn revisions(
    &self,
  ) -> impl std::future::Future<
    Output = Result<qobuz_api::library_pages::LibraryRevisions, qobuz_api::ApiError>,
  > + Send {
    self.library_revisions()
  }
  fn source_page(
    &self,
    source: LibrarySource,
    account: u64,
    offset: u64,
    limit: u32,
  ) -> impl std::future::Future<Output = Result<crate::library_sources::SourcePage, qobuz_api::ApiError>>
       + Send {
    source.fetch_page(self, account, offset, limit)
  }
}

const FRESH_SECONDS: i64 = 300;
const SYNC_PAGE: u32 = 500;
const STEP_PAGES: usize = 4;
const STEP_SEED_CHUNKS: usize = 128;
const STEP_BUDGET: Duration = Duration::from_secs(2);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE_BUDGET: Duration = Duration::from_secs(6);
const SETTLE_STEPS: usize = 24;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Filters {
  genres: Vec<String>,
  genre: String,
  membership: String,
  favorite: bool,
  followed: bool,
  purchased: bool,
  hide_purchases: bool,
  hires: bool,
  owner: String,
  text: String,
}

impl Filters {
  fn membership(&self) -> &str {
    if !self.membership.is_empty() {
      return &self.membership;
    }
    match (self.favorite, self.purchased, self.followed) {
      (true, false, false) => "favorites",
      (false, true, false) => "purchases",
      (false, false, true) => "followed",
      _ => "all",
    }
  }

  fn presented(&self) -> Value {
    let mut filters = serde_json::to_value(self).unwrap_or_default();
    filters["membership"] = json!(self.membership());
    if self.owner.is_empty() {
      filters["owner"] = json!("all");
    }
    if self.genre.is_empty() && self.genres.len() == 1 {
      filters["genre"] = json!(self.genres[0]);
    }
    filters
  }
}

#[derive(Clone, Serialize, Deserialize)]
struct Selection {
  view: String,
  filters: Filters,
  order: Order,
  cursor: Option<String>,
  generation: Option<i64>,
}

impl Selection {
  fn read(request: &Value) -> Result<Self, RpcFault> {
    let reference = request["ref"].as_str().unwrap_or("library:all");
    let mut selection = if let Some(encoded) = reference.strip_prefix("library:v1:") {
      if encoded.len() > 16384 {
        return Err(RpcFault::invalid("library reference"));
      }
      let bytes =
        URL_SAFE_NO_PAD.decode(encoded).map_err(|_| RpcFault::invalid("library reference"))?;
      serde_json::from_slice::<Self>(&bytes).map_err(|_| RpcFault::invalid("library reference"))?
    } else {
      Self {
        view: reference.strip_prefix("library:").unwrap_or("all").into(),
        filters: Filters::default(),
        order: if reference == "library:awards" { Order::Title } else { Order::Added },
        cursor: None,
        generation: None,
      }
    };
    if request["filters"].is_object() {
      selection.filters = serde_json::from_value(request["filters"].clone())
        .map_err(|_| RpcFault::invalid("library filters"))?;
      if request["filters"].get("membership").is_some() && !selection.filters.membership.is_empty()
      {
        selection.filters.favorite = false;
        selection.filters.followed = false;
        selection.filters.purchased = false;
      }
      if request["filters"].get("genre").is_some() {
        selection.filters.genres.clear();
      }
      selection.cursor = None;
      selection.generation = None;
    }
    if let Some(order) = request["order"].as_str() {
      selection.order = match order {
        "added" => Order::Added,
        "title" => Order::Title,
        _ => return Err(RpcFault::invalid("library order")),
      };
      selection.cursor = None;
      selection.generation = None;
    }
    selection.kind()?;
    if selection.filters.text.len() > 256
      || selection.filters.owner.len() > 64
      || selection.filters.genre.len() > 64
      || selection.filters.genres.len() > 64
      || selection.filters.genres.iter().any(|genre| genre.len() > 64)
    {
      return Err(RpcFault::invalid("library filters"));
    }
    if !matches!(
      selection.filters.membership.as_str(),
      "" | "all" | "favorites" | "purchases" | "followed"
    ) {
      return Err(RpcFault::invalid("library membership"));
    }
    if (selection.filters.purchased || selection.filters.membership == "purchases")
      && selection.filters.hide_purchases
    {
      return Err(RpcFault::invalid("conflicting purchase filters"));
    }
    Ok(selection)
  }

  fn kind(&self) -> Result<Option<&'static str>, RpcFault> {
    match self.view.as_str() {
      "all" => Ok(None),
      "playlists" => Ok(Some("playlist")),
      "tracks" => Ok(Some("track")),
      "albums" => Ok(Some("album")),
      "artists" => Ok(Some("artist")),
      "labels" => Ok(Some("label")),
      "awards" => Ok(Some("award")),
      _ => Err(RpcFault::invalid("library category")),
    }
  }

  fn query(&self, account: u64) -> Result<Query, RpcFault> {
    let mut query = Query {
      order: self.order,
      text: self.filters.text.clone(),
      genres: self.filters.genres.clone(),
      explicit_only: self.view == "all",
      exclude_purchases: self.filters.hide_purchases,
      ..Default::default()
    };
    if !self.filters.genre.is_empty() {
      query.genres = vec![self.filters.genre.clone()];
    }
    if let Some(kind) = self.kind()? {
      query.kinds.push(kind.into());
    }
    if self.filters.favorite {
      query.membership_groups.push(vec![Membership::Favorites]);
    }
    if self.filters.followed {
      query.membership_groups.push(vec![Membership::Followed]);
    }
    if self.filters.purchased {
      query.membership_groups.push(vec![Membership::Purchases]);
    }
    match self.filters.membership.as_str() {
      "favorites" => query.membership_groups.push(vec![Membership::Favorites]),
      "purchases" => query.membership_groups.push(vec![Membership::Purchases]),
      "followed" => query.membership_groups.push(vec![Membership::Followed]),
      _ => (),
    }
    if self.filters.hires {
      query.hires = Some(true);
    }
    match self.filters.owner.as_str() {
      "" | "all" => (),
      "me" => query.owners.push(account.to_string()),
      "qobuz" => query.owners.push("0".into()),
      "other" => query.exclude_owners = vec![account.to_string(), "0".into()],
      value if value.bytes().all(|byte| byte.is_ascii_digit()) => query.owners.push(value.into()),
      _ => return Err(RpcFault::invalid("playlist owner")),
    }
    Ok(query)
  }

  fn reference(&self) -> Result<String, RpcFault> {
    let bytes = serde_json::to_vec(self).map_err(|_| RpcFault::invalid("library reference"))?;
    Ok(format!("library:v1:{}", URL_SAFE_NO_PAD.encode(bytes)))
  }
}

struct Sync {
  generation: Generation,
  source: usize,
  offset: u64,
  loaded: u64,
  source_total: Option<u64>,
  page_size: u32,
  changed: [bool; 8],
  revisions: qobuz_api::library_pages::LibraryRevisions,
  mutations: [u64; 8],
  seed: Option<Seed>,
}

struct Seed {
  replaced: Vec<(Membership, String)>,
  cursor: Option<String>,
}

#[derive(Default)]
struct Progress {
  epoch: u64,
  work: Option<Sync>,
  cleaning: bool,
  dirty: bool,
  planned: Option<qobuz_api::library_pages::LibraryRevisions>,
  retry_at: Option<Instant>,
  last_error: Option<String>,
  applied: [u64; 8],
  retired: Option<Generation>,
}

struct AccountIndex {
  account: u64,
  index: LibraryIndex,
  progress: AsyncMutex<Progress>,
  mutations: [AtomicU64; 8],
}

impl AccountIndex {
  fn new(account: u64, index: LibraryIndex, epoch: u64) -> Result<Self, RpcFault> {
    let versions = index.local_versions().map_err(fault)?;
    let dirty = index.projection_pending().map_err(fault)?;
    let mutations = std::array::from_fn(|position| {
      AtomicU64::new(versions.get(LibrarySource::ALL[position].name()).map_or(0, |v| v.version))
    });
    let applied = std::array::from_fn(|position| {
      versions.get(LibrarySource::ALL[position].name()).map_or(0, |v| v.applied)
    });
    Ok(Self {
      account,
      index,
      mutations,
      progress: AsyncMutex::new(Progress { epoch, applied, dirty, ..Default::default() }),
    })
  }

  fn mutation_versions(&self) -> [u64; 8] {
    std::array::from_fn(|position| self.mutations[position].load(AtomicOrdering::Acquire))
  }

  fn changed(&self, source: LibrarySource) -> Result<(), RpcFault> {
    let version = self.index.mark_local_changed(source.name()).map_err(fault)?;
    let position =
      LibrarySource::ALL.iter().position(|entry| *entry == source).expect("library source");
    self.mutations[position].fetch_max(version, AtomicOrdering::AcqRel);
    Ok(())
  }
}

#[derive(Default)]
pub(crate) struct LibraryCatalog {
  epoch: AtomicU64,
  mutations: Arc<RwLock<()>>,
  account: Mutex<Option<Arc<AccountIndex>>>,
  genres: LibraryGenres,
}

pub(crate) struct LibraryMutation {
  _guard: OwnedRwLockWriteGuard<()>,
}

/// Names the collection and offset a synchronization failure came from, so a
/// report says where the service answered something unexpected.
fn located(source: LibrarySource, offset: u64, error: qobuz_api::ApiError) -> qobuz_api::ApiError {
  match error {
    qobuz_api::ApiError::ApiResponse(message) => qobuz_api::ApiError::ApiResponse(format!(
      "{message} ({} at offset {offset})",
      source.name()
    )),
    other => other,
  }
}

fn membership(source: LibrarySource) -> Membership {
  match source.membership() {
    "purchased" => Membership::Purchases,
    "followed" => Membership::Followed,
    "subscribed" => Membership::Subscribed,
    _ => Membership::Favorites,
  }
}

fn fault(error: String) -> RpcFault {
  RpcFault::new(-32000, error)
}

impl LibraryCatalog {
  fn pending_mutations(account: &AccountIndex, progress: &Progress) -> bool {
    account.mutation_versions() != progress.applied
  }

  pub async fn changed(
    &self,
    session: &SessionContext,
    source: LibrarySource,
  ) -> Result<LibraryMutation, RpcFault> {
    let user = session.api.session().await.ok_or_else(|| RpcFault::new(-32001, "Sign in first"))?;
    self.changed_account(session, user.user_id, source).await
  }

  async fn changed_account(
    &self,
    session: &SessionContext,
    account: u64,
    source: LibrarySource,
  ) -> Result<LibraryMutation, RpcFault> {
    let guard = self.mutations.clone().write_owned().await;
    if LibraryIndex::exists(&session.cache, account).map_err(fault)? {
      self.account(session, account)?.changed(source)?;
    }
    self.invalidate();
    Ok(LibraryMutation { _guard: guard })
  }

  pub async fn favorite_changed(
    &self,
    session: &SessionContext,
    kind: &str,
  ) -> Result<LibraryMutation, RpcFault> {
    let source = match kind {
      "album" => LibrarySource::FavoriteAlbums,
      "track" => LibrarySource::FavoriteTracks,
      "artist" => LibrarySource::FollowedArtists,
      "label" => LibrarySource::FollowedLabels,
      "award" => LibrarySource::FollowedAwards,
      _ => return Err(RpcFault::invalid("favorite kind")),
    };
    self.changed(session, source).await
  }

  pub async fn playlists_changed(
    &self,
    session: &SessionContext,
  ) -> Result<LibraryMutation, RpcFault> {
    self.changed(session, LibrarySource::Playlists).await
  }

  pub fn invalidate(&self) {
    self.epoch.fetch_add(1, AtomicOrdering::Relaxed);
  }

  /// Brings the index up to an accepted write before its action answers, so
  /// the page read next already holds the new state. Best effort and bounded:
  /// the write stands either way and a later read finishes what is left.
  pub async fn settle(&self, session: &SessionContext) {
    let Some(user) = session.api.session().await else {
      return;
    };
    if !session.network.load(AtomicOrdering::Relaxed)
      || !LibraryIndex::exists(&session.cache, user.user_id).unwrap_or(false)
    {
      return;
    }
    let Ok(account) = self.account(session, user.user_id) else {
      return;
    };
    let deadline = Instant::now() + SETTLE_BUDGET;
    let mut progress = account.progress.lock().await;
    for _ in 0..SETTLE_STEPS {
      if self.advance(&session.api, &account, &mut progress, false).await.is_err()
        || Instant::now() >= deadline
        || progress.retry_at.is_some_and(|due| due > Instant::now())
      {
        return;
      }
      if progress.work.is_none()
        && !progress.dirty
        && progress.planned.is_none()
        && !Self::pending_mutations(&account, &progress)
      {
        return;
      }
    }
  }

  pub fn reset(&self) {
    self.account.lock().unwrap_or_else(|poison| poison.into_inner()).take();
    self.invalidate();
  }

  fn account(&self, session: &SessionContext, account: u64) -> Result<Arc<AccountIndex>, RpcFault> {
    let mut current = self.account.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(current) = current.as_ref().filter(|current| current.account == account) {
      return Ok(current.clone());
    }
    let index = LibraryIndex::open(&session.cache, account).map_err(fault)?;
    let found =
      Arc::new(AccountIndex::new(account, index, self.epoch.load(AtomicOrdering::Relaxed))?);
    *current = Some(found.clone());
    Ok(found)
  }

  fn defer(progress: &mut Progress, error: qobuz_api::ApiError) -> RpcFault {
    let seconds = match &error {
      qobuz_api::ApiError::RateLimited(seconds)
      | qobuz_api::ApiError::ForbiddenCircuitOpen(seconds) => (*seconds).clamp(1, 3600),
      _ => 10,
    };
    progress.retry_at = Some(Instant::now() + Duration::from_secs(seconds));
    progress.last_error = Some(error.to_string());
    let transient = error.is_transient()
      || matches!(
        error,
        qobuz_api::ApiError::OfflineMode | qobuz_api::ApiError::ForbiddenCircuitOpen(_)
      );
    let mut fault: RpcFault = error.into();
    if transient {
      fault.data = Some(json!({"retry_after_ms":seconds * 1000,"transient":true}));
    }
    fault
  }

  fn discard(
    account: &AccountIndex,
    progress: &mut Progress,
    generation: Option<Generation>,
    error: qobuz_api::ApiError,
  ) -> RpcFault {
    let generation =
      progress.work.take().map(|work| work.generation).or(generation).or(progress.retired);
    progress.planned = None;
    progress.dirty = true;
    progress.retired = generation;
    let mut fault = Self::defer(progress, error);
    if let Some(generation) = generation {
      match account.index.abandon(generation) {
        Ok(()) => progress.retired = None,
        Err(error) => fault.message.push_str(&format!("; snapshot cleanup failed: {error}")),
      }
    }
    progress.last_error = Some(fault.message.clone());
    fault
  }

  fn refresh_delay(updating: bool, retry_after_ms: Option<u64>) -> Option<u64> {
    updating.then(|| retry_after_ms.unwrap_or(250).max(250))
  }

  fn refresh_parents(account: &AccountIndex) -> Result<(), RpcFault> {
    for source in LibrarySource::ALL {
      if matches!(source.kind(), "album" | "track") {
        account.changed(source)?;
      }
    }
    Ok(())
  }

  async fn revisions(
    remote: &impl LibraryRemote,
  ) -> Result<qobuz_api::library_pages::LibraryRevisions, qobuz_api::ApiError> {
    match tokio::time::timeout(NETWORK_TIMEOUT, remote.revisions()).await {
      Ok(result) => result,
      Err(_) => Err(qobuz_api::ApiError::ServerError(504)),
    }
  }

  fn seed_snapshot(
    account: &AccountIndex,
    progress: &mut Progress,
    started: Instant,
    chunks: usize,
  ) -> Result<bool, RpcFault> {
    for _ in 0..chunks {
      if started.elapsed() >= STEP_BUDGET {
        return Ok(false);
      }
      let work = progress.work.as_mut().expect("library synchronization");
      let Some(seed) = work.seed.as_mut() else {
        return Ok(true);
      };
      let result = account.index.seed_from_current_chunk(
        work.generation,
        &seed.replaced,
        seed.cursor.as_deref(),
        500,
      );
      let page = match result {
        Ok(page) => page,
        Err(error) => {
          return Err(Self::discard(
            account,
            progress,
            None,
            qobuz_api::ApiError::ApiResponse(error),
          ))
        }
      };
      work.loaded += page.stats.new_entities;
      if page.done {
        work.seed = None;
        return Ok(true);
      }
      if page.next.is_none() || page.next == seed.cursor {
        return Err(Self::discard(
          account,
          progress,
          None,
          qobuz_api::ApiError::ApiResponse("The library snapshot copy did not advance".into()),
        ));
      }
      seed.cursor = page.next;
    }
    Ok(false)
  }

  async fn advance(
    &self,
    remote: &impl LibraryRemote,
    account: &AccountIndex,
    progress: &mut Progress,
    refresh: bool,
  ) -> Result<(), RpcFault> {
    let Ok(_mutation_guard) = self.mutations.try_read() else {
      progress.dirty = true;
      return Ok(());
    };
    let epoch = self.epoch.load(AtomicOrdering::Relaxed);
    if refresh || progress.epoch != epoch {
      progress.epoch = epoch;
      progress.dirty = true;
      if progress.work.is_none() {
        progress.planned = None;
      }
      if refresh {
        progress.retry_at = None;
      }
    }
    if progress.retry_at.is_some_and(|due| due > Instant::now()) {
      return Ok(());
    }
    if let Some(generation) = progress.retired {
      account
        .index
        .abandon(generation)
        .map_err(|error| Self::defer(progress, qobuz_api::ApiError::ApiResponse(error)))?;
      progress.retired = None;
    }
    if Self::pending_mutations(account, progress) {
      progress.dirty = true;
    }
    let status = account.index.status().map_err(fault)?;
    let projection_pending = account.index.projection_pending().map_err(fault)?;
    if projection_pending {
      progress.dirty = true;
    }
    if progress.work.is_none()
      && !progress.dirty
      && status.generation > 0
      && (0..FRESH_SECONDS).contains(&(crate::catalog::now() as i64 - status.checked_at))
    {
      return Ok(());
    }
    let started = Instant::now();
    if progress.work.is_none() {
      if progress.planned.is_none() {
        let revisions =
          Self::revisions(remote).await.map_err(|error| Self::defer(progress, error))?;
        let previous = account.index.revisions().map_err(fault)?;
        if !projection_pending
          && status.generation > 0
          && previous == revisions
          && !Self::pending_mutations(account, progress)
        {
          account.index.mark_checked(crate::catalog::now() as i64).map_err(fault)?;
          progress.dirty = Self::pending_mutations(account, progress);
          progress.cleaning = false;
          progress.retry_at = None;
          progress.last_error = None;
          return Ok(());
        }
        progress.planned = Some(revisions);
        progress.dirty = true;
      }
      if started.elapsed() >= STEP_BUDGET {
        return Ok(());
      }
      let mut cleaned = 0;
      while cleaned < 4000 {
        let removed = account.index.prune_chunk(500).map_err(fault)?;
        cleaned += removed.removed;
        progress.cleaning = removed.pending;
        if !removed.pending {
          break;
        }
        if started.elapsed() >= STEP_BUDGET {
          return Ok(());
        }
      }
      if progress.cleaning {
        return Ok(());
      }
      let revisions = progress.planned.take().expect("planned library synchronization");
      let previous = account.index.revisions().map_err(fault)?;
      let mutations = account.mutation_versions();
      let changed = std::array::from_fn(|position| {
        let name = LibrarySource::ALL[position].name();
        status.generation == 0
          || previous.get(name) != revisions.get(name)
          || mutations[position] != progress.applied[position]
      });
      if !projection_pending && !changed.iter().any(|changed| *changed) {
        account.index.mark_checked(crate::catalog::now() as i64).map_err(fault)?;
        progress.dirty = Self::pending_mutations(account, progress);
        progress.cleaning = false;
        progress.retry_at = None;
        progress.last_error = None;
        return Ok(());
      }
      let expected = [Membership::Favorites, Membership::Purchases, Membership::Followed];
      let generation =
        account.index.begin_generation(&expected, crate::catalog::now() as i64).map_err(fault)?;
      let seed = if status.generation > 0 && changed.iter().any(|changed| !*changed) {
        let replaced: Vec<_> = LibrarySource::ALL
          .iter()
          .zip(changed)
          .filter(|(_, changed)| *changed)
          .map(|(source, _)| (membership(*source), source.kind().to_string()))
          .collect();
        Some(Seed { replaced, cursor: None })
      } else {
        None
      };
      progress.work = Some(Sync {
        generation,
        source: 0,
        offset: 0,
        loaded: 0,
        source_total: None,
        page_size: SYNC_PAGE,
        changed,
        revisions,
        mutations,
        seed,
      });
    }
    if !Self::seed_snapshot(account, progress, started, STEP_SEED_CHUNKS)? {
      return Ok(());
    }
    for _ in 0..STEP_PAGES {
      let work = progress.work.as_mut().expect("library synchronization");
      while work.source < LibrarySource::ALL.len() && !work.changed[work.source] {
        work.source += 1;
      }
      if work.source == LibrarySource::ALL.len() {
        if started.elapsed() >= STEP_BUDGET {
          return Ok(());
        }
        let current =
          Self::revisions(remote).await.map_err(|error| Self::defer(progress, error))?;
        let work = progress.work.as_ref().expect("library synchronization");
        let mutations = work.mutations;
        let changed_during_sync =
          current != work.revisions || account.mutation_versions() != mutations;
        let publication = (|| -> Result<(), String> {
          for collection in [Membership::Favorites, Membership::Purchases, Membership::Followed] {
            account.index.finish_collection(work.generation, collection)?;
          }
          account.index.stage_revisions(work.generation, &work.revisions)?;
          let captured = LibrarySource::ALL
            .iter()
            .zip(mutations)
            .map(|(source, version)| (source.name().to_string(), version))
            .collect();
          account.index.publish_with_local_versions(
            work.generation,
            if changed_during_sync { 0 } else { crate::catalog::now() as i64 },
            &captured,
          )?;
          Ok(())
        })();
        if let Err(error) = publication {
          return Err(Self::discard(
            account,
            progress,
            None,
            qobuz_api::ApiError::ApiResponse(error),
          ));
        }
        progress.work = None;
        progress.applied = mutations;
        progress.dirty = changed_during_sync || Self::pending_mutations(account, progress);
        progress.cleaning = false;
        progress.retry_at = None;
        progress.last_error = None;
        return Ok(());
      }
      let source = LibrarySource::ALL[work.source];
      if started.elapsed() >= STEP_BUDGET {
        break;
      }
      let page = match tokio::time::timeout(
        NETWORK_TIMEOUT,
        remote.source_page(source, account.account, work.offset, work.page_size),
      )
      .await
      {
        Ok(Ok(page)) => page,
        Ok(Err(error))
          if qobuz_api::library_pages::response_too_large(&error) && work.page_size > 25 =>
        {
          work.page_size = (work.page_size / 2).max(25);
          continue;
        }
        Ok(Err(error))
          if error.is_transient()
            || matches!(
              error,
              qobuz_api::ApiError::OfflineMode | qobuz_api::ApiError::ForbiddenCircuitOpen(_)
            ) =>
        {
          return Err(Self::defer(progress, error))
        }
        Ok(Err(error)) => {
          let error = located(source, work.offset, error);
          return Err(Self::discard(account, progress, None, error));
        }
        Err(_) => return Err(Self::defer(progress, qobuz_api::ApiError::ServerError(504))),
      };
      if page.total.is_some_and(|total| work.source_total.is_some_and(|before| before != total)) {
        let error = qobuz_api::ApiError::ApiResponse(
          "The library collection total changed during synchronization".into(),
        );
        let error = located(source, work.offset, error);
        return Err(Self::discard(account, progress, None, error));
      }
      let rows: Vec<_> = page
        .items
        .iter()
        .map(|row| IndexedItem { item: &row.card, added_at: row.added_at, explicit: row.explicit })
        .collect();
      let stats = match account.index.ingest(work.generation, membership(source), &rows) {
        Ok(stats) => stats,
        Err(error) => {
          let error = located(source, work.offset, qobuz_api::ApiError::ApiResponse(error));
          return Err(Self::discard(account, progress, None, error));
        }
      };
      work.source_total = page.total.or(work.source_total);
      work.loaded += stats.new_memberships;
      // An item an earlier page already brought is skipped, not fatal: the
      // service repeats rows across pages in some accounts, and the count
      // need not match its total. A page that brings nothing new ends the
      // collection, so a service that ignores the offset cannot loop.
      let stalled = !page.items.is_empty() && stats.new_memberships == 0;
      match page.next_offset.filter(|_| !stalled) {
        Some(offset) => work.offset = offset,
        None => {
          work.source += 1;
          work.offset = 0;
          work.source_total = None;
          work.page_size = SYNC_PAGE;
        }
      }
      progress.retry_at = None;
      progress.last_error = None;
      if started.elapsed() >= STEP_BUDGET {
        break;
      }
    }
    Ok(())
  }

  pub async fn page(&self, session: &SessionContext, request: Value) -> RpcResult {
    self.page_with_contract(session, request, false).await
  }

  async fn advance_visible(
    &self,
    remote: &impl LibraryRemote,
    account: &AccountIndex,
    progress: &mut Progress,
    cursor: bool,
    refresh: bool,
  ) -> Result<(), RpcFault> {
    if !cursor
      || progress.work.is_some()
      || progress.dirty
      || progress.cleaning
      || progress.planned.is_some()
      || progress.retired.is_some()
      || progress.epoch != self.epoch.load(AtomicOrdering::Relaxed)
      || Self::pending_mutations(account, progress)
    {
      self.advance(remote, account, progress, refresh && !cursor).await?;
    }
    Ok(())
  }

  pub async fn page_with_contract(
    &self,
    session: &SessionContext,
    request: Value,
    collection: bool,
  ) -> RpcResult {
    session.require_identity().await?;
    let user = session.api.session().await.ok_or_else(|| RpcFault::new(-32001, "Sign in first"))?;
    let mut selection = Selection::read(&request)?;
    let continuation = selection.cursor.is_some();
    let account = self.account(session, user.user_id)?;
    let mut update_error = None;
    let online = session.network.load(AtomicOrdering::Relaxed);
    if online {
      if !continuation && request["refresh"] == true {
        Self::refresh_parents(&account)?;
      }
      if let Ok(mut progress) = account.progress.try_lock() {
        if let Err(error) = self
          .advance_visible(
            &session.api,
            &account,
            &mut progress,
            continuation,
            request["refresh"] == true,
          )
          .await
        {
          if account.index.status().map_err(fault)?.generation == 0 && selection.view != "awards" {
            return Err(error);
          }
          update_error = Some(error.message);
        }
      }
    }
    let query = selection.query(user.user_id)?;
    let limit = request["limit"].as_u64().unwrap_or(100).clamp(1, 100) as usize;
    let stale = |mut selection: Selection| {
      selection.cursor = None;
      selection.generation = None;
      RpcFault {
        code: -32010,
        message: "The library changed; reload the first page".into(),
        data: Some(json!({"stale_cursor":true,"restart_ref":selection.reference().ok()})),
      }
    };
    if selection.generation.is_some_and(|generation| {
      account.index.status().is_ok_and(|status| status.generation != generation)
    }) {
      return Err(stale(selection));
    }
    let awards = if selection.view == "awards" {
      Some(
        account
          .index
          .awards_page(
            &session.api,
            &query,
            selection.cursor.as_deref(),
            limit,
            online,
            request["refresh"] == true && !continuation,
          )
          .await,
      )
    } else {
      None
    };
    let (page_result, award_details) = match awards {
      Some(Ok(awards)) => {
        (Ok(awards.page), Some((awards.total, awards.catalog_revision, awards.stale, awards.error)))
      }
      Some(Err(error)) => (Err(error), None),
      None => (account.index.query(&query, selection.cursor.as_deref(), limit), None),
    };
    let mut page = match page_result {
      Ok(page) => page,
      Err(error) if error == "The library cursor is stale; reload the first page" => {
        return Err(stale(selection))
      }
      Err(error) => return Err(fault(error)),
    };
    let _ = crate::library_index::artwork::enrich(
      &account.index,
      &session.api,
      &mut page.items,
      online,
      request["refresh"] == true && !continuation,
    )
    .await;
    for item in &mut page.items {
      crate::projection::image_fallback(item);
    }
    let next = if let Some(cursor) = page.next {
      selection.cursor = Some(cursor);
      selection.generation = Some(page.status.generation);
      Some(selection.reference()?)
    } else {
      None
    };
    let progress = account.progress.try_lock().ok();
    let updating = online
      && progress.as_ref().is_none_or(|progress| {
        progress.work.is_some()
          || progress.cleaning
          || progress.dirty
          || Self::pending_mutations(&account, progress)
      });
    let loaded =
      progress.as_ref().and_then(|progress| progress.work.as_ref()).map(|work| work.loaded);
    if update_error.is_none() {
      update_error = progress.as_ref().and_then(|progress| progress.last_error.clone());
    }
    let retry_after_ms = progress
      .as_ref()
      .and_then(|progress| progress.retry_at)
      .map(|due| due.saturating_duration_since(Instant::now()).as_millis().min(3600000) as u64);
    let spanish = session.language().as_deref() == Some("es");
    let tabs: Vec<_> = crate::library_sources::LIBRARY_VIEWS
      .into_iter().map(|(id,en,es)| json!({"title":if spanish {es} else {en},"ref":format!("library:{id}"),"active":id==selection.view})).collect();
    let mut reply = json!({"title":if spanish {"Biblioteca"} else {"Library"},"library":true,"view":selection.view,
      "layout":"mixed","tabs":tabs,"items":page.items,"next":next,"has_more":page.has_more,
      "revision":format!("{}:{}",page.status.generation,page.status.revision),"loading":page.status.generation==0,
      "updating":updating,"sync_loaded":loaded,"refresh_after_ms":Self::refresh_delay(updating,retry_after_ms),
      "error":update_error,"offline":!online,"retry_after_ms":retry_after_ms,"filters":selection.filters,"order":selection.order});
    if let Some((total, revision, stale, error)) = award_details {
      reply["sync_loaded"] = json!(total);
      reply["loading"] = json!(false);
      reply["revision"] =
        json!(format!("{}:{}:{revision}", page.status.generation, page.status.revision));
      reply["stale"] = json!(stale);
      if let Some(error) = error {
        reply["error"] = json!(error);
      }
    }
    if collection {
      drop(progress);
      let genres = if matches!(selection.view.as_str(), "all" | "albums" | "tracks") {
        self.genres.options(&session.api, spanish, online && !continuation).await
      } else {
        vec![]
      };
      reply["title"] = json!(crate::library_sources::library_title(&selection.view, spanish));
      reply["layout"] = json!("collection");
      reply["query_ref"] = json!(format!("library:{}", selection.view));
      reply["show_tabs"] = json!(selection.view == "all");
      reply["filters"] = selection.filters.presented();
      if selection.view == "awards"
        && matches!(selection.filters.membership(), "favorites" | "followed")
      {
        reply["filters"]["membership"] = json!("followed");
      }
      reply["filter_controls"] = json!(crate::library_sources::filter_controls(
        &selection.view,
        spanish,
        selection.filters.membership() == "purchases",
        &genres
      ));
      reply["order_options"] = json!([
        {"value":"added","label":if spanish {"Fecha de añadido"} else {"Date added"}},
        {"value":"title","label":if spanish {"Título"} else {"Title"}},
      ]);
      reply["actions"] = json!({"favorite":true});
      reply["playlist_edit"] = json!(selection.view == "playlists");
    }
    Ok(reply)
  }
}

#[cfg(test)]
#[path = "checks/library_catalog.rs"]
mod tests;
