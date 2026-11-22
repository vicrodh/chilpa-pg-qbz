use super::*;
use crate::library_sources::{SourcePage, SourceRow};
use std::collections::{BTreeMap, VecDeque};

struct RemoteFixture {
  revisions: Mutex<qobuz_api::library_pages::LibraryRevisions>,
  pages: Mutex<VecDeque<Result<SourcePage, qobuz_api::ApiError>>>,
  calls: Mutex<Vec<(LibrarySource, u64)>>,
  delay_once: Mutex<Option<Duration>>,
  revision_calls: AtomicU64,
}
impl RemoteFixture {
  fn new() -> Self {
    Self {
      revisions: Mutex::new(
        LibrarySource::ALL
          .into_iter()
          .map(|source| (source.name().into(), Some(1)))
          .collect::<BTreeMap<_, _>>(),
      ),
      pages: Mutex::new(VecDeque::new()),
      calls: Mutex::new(Vec::new()),
      delay_once: Mutex::new(None),
      revision_calls: AtomicU64::new(0),
    }
  }
  fn enqueue(&self, page: Result<SourcePage, qobuz_api::ApiError>) {
    self.pages.lock().unwrap().push_back(page);
  }
}
impl LibraryRemote for RemoteFixture {
  async fn revisions(
    &self,
  ) -> Result<qobuz_api::library_pages::LibraryRevisions, qobuz_api::ApiError> {
    self.revision_calls.fetch_add(1, AtomicOrdering::Relaxed);
    Ok(self.revisions.lock().unwrap().clone())
  }
  async fn source_page(
    &self,
    source: LibrarySource,
    _: u64,
    offset: u64,
    _: u32,
  ) -> Result<SourcePage, qobuz_api::ApiError> {
    self.calls.lock().unwrap().push((source, offset));
    let delay = self.delay_once.lock().unwrap().take();
    if let Some(delay) = delay {
      tokio::time::sleep(delay).await;
    }
    self
      .pages
      .lock()
      .unwrap()
      .pop_front()
      .unwrap_or_else(|| Ok(SourcePage { items: vec![], next_offset: None, total: Some(0) }))
  }
}
fn account(directory: &std::path::Path) -> AccountIndex {
  AccountIndex::new(7, LibraryIndex::open(directory, 7).unwrap(), 0).unwrap()
}
fn album(id: &str, next_offset: Option<u64>, total: u64) -> SourcePage {
  SourcePage {
    items: vec![SourceRow {
      card: json!({"kind":"album","ref":format!("album:{id}"),"title":id}),
      added_at: 10,
      explicit: true,
    }],
    next_offset,
    total: Some(total),
  }
}

#[tokio::test]
async fn cursor_polls_continue_pending_sync_and_publish_a_stale_cursor() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  let mut saved = album("saved-a", None, 2);
  saved.items.extend(album("saved-b", None, 2).items);
  remote.enqueue(Ok(saved));
  finish(&catalog, &remote, &account, &mut progress).await;
  let query = Query::default();
  let cursor = account.index.query(&query, None, 1).unwrap().next.unwrap();
  let before = account.index.status().unwrap();
  remote.revisions.lock().unwrap().insert(LibrarySource::FavoriteAlbums.name().into(), Some(2));
  for n in 0..5 {
    remote.enqueue(Ok(album(&format!("fresh-{n}"), (n < 4).then_some(n + 1), 5)));
  }
  catalog.invalidate();
  catalog.advance_visible(&remote, &account, &mut progress, false, false).await.unwrap();
  assert_eq!(progress.work.as_ref().unwrap().offset, 4);
  assert_eq!(account.index.status().unwrap().generation, before.generation);
  assert_eq!(
    account.index.query(&query, Some(&cursor), 1).unwrap().items[0]["ref"],
    "album:saved-b"
  );
  catalog.advance_visible(&remote, &account, &mut progress, true, true).await.unwrap();
  assert!(progress.work.is_none(), "A visible cursor page must continue pending synchronization");
  assert_eq!(account.index.status().unwrap().items, 5);
  assert!(account.index.status().unwrap().generation != before.generation);
  assert_eq!(
    account.index.query(&query, Some(&cursor), 1).unwrap_err(),
    "The library cursor is stale; reload the first page"
  );
  assert_eq!(
    account.mutation_versions(),
    [0; 8],
    "Cursor polling must not request a manual refresh"
  );
}

#[tokio::test]
async fn an_idle_cursor_does_not_revalidate_or_fetch_even_after_freshness_expires() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  finish(&catalog, &remote, &account, &mut progress).await;
  account.index.mark_checked(0).unwrap();
  let calls = remote.calls.lock().unwrap().len();
  let revisions = remote.revision_calls.load(AtomicOrdering::Relaxed);
  catalog.advance_visible(&remote, &account, &mut progress, true, true).await.unwrap();
  assert_eq!(remote.calls.lock().unwrap().len(), calls);
  assert_eq!(remote.revision_calls.load(AtomicOrdering::Relaxed), revisions);
  assert_eq!(account.mutation_versions(), [0; 8]);
}
async fn finish(
  catalog: &LibraryCatalog,
  remote: &RemoteFixture,
  account: &AccountIndex,
  progress: &mut Progress,
) {
  for _ in 0..10 {
    catalog.advance(remote, account, progress, false).await.unwrap();
    if progress.work.is_none() && account.index.status().unwrap().generation > 0 {
      return;
    }
  }
  panic!("The fixture synchronization did not finish");
}

#[tokio::test]
async fn unchanged_revisions_skip_remote_collections_and_only_changed_sources_reload() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(remote.calls.lock().unwrap().len(), 8);
  let before = account.index.status().unwrap();
  progress.cleaning = true;
  catalog.invalidate();
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(
    remote.calls.lock().unwrap().len(),
    8,
    "Unchanged service revisions must not redownload the collection"
  );
  assert_eq!(account.index.status().unwrap().generation, before.generation);
  assert!(!progress.cleaning);
  remote.revisions.lock().unwrap().insert(LibrarySource::FavoriteAlbums.name().into(), Some(2));
  remote.enqueue(Ok(album("new", None, 1)));
  catalog.invalidate();
  finish(&catalog, &remote, &account, &mut progress).await;
  let calls = remote.calls.lock().unwrap();
  assert_eq!(&calls[8..], &[(LibrarySource::FavoriteAlbums, 0)]);
  assert_eq!(account.index.status().unwrap().items, 1);
}

#[tokio::test]
async fn an_older_projection_rebuilds_from_cache_without_downloading_unchanged_sources() {
  let directory = tempfile::tempdir().unwrap();
  let remote = RemoteFixture::new();
  {
    let account = account(directory.path());
    let catalog = LibraryCatalog::default();
    remote.enqueue(Ok(album("saved", None, 1)));
    finish(&catalog, &remote, &account, &mut Progress::default()).await;
  }
  let calls = remote.calls.lock().unwrap().len();
  {
    let connection =
      rusqlite::Connection::open(directory.path().join("library-7.sqlite3")).unwrap();
    connection.execute("UPDATE metadata SET projection_version=1 WHERE id=1", []).unwrap();
  }
  let account = account(directory.path());
  assert!(account.index.projection_pending().unwrap());
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  finish(&catalog, &remote, &account, &mut progress).await;
  assert!(!account.index.projection_pending().unwrap());
  assert_eq!(remote.calls.lock().unwrap().len(), calls);
  assert_eq!(
    account.index.query(&Query::default(), None, 100).unwrap().items[0]["ref"],
    "album:saved"
  );
}

#[tokio::test]
async fn manual_refresh_enriches_parent_sources_even_when_remote_tokens_match() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  remote.enqueue(Ok(album("saved", None, 1)));
  finish(&catalog, &remote, &account, &mut progress).await;
  let before = account.index.status().unwrap();
  let calls = remote.calls.lock().unwrap().len();
  let mut enriched = album("saved", None, 1);
  enriched.items[0].card["title"] = json!("Enriched record");
  remote.enqueue(Ok(enriched));
  LibraryCatalog::refresh_parents(&account).unwrap();
  catalog.advance(&remote, &account, &mut progress, true).await.unwrap();
  assert_eq!(account.index.status().unwrap().generation, before.generation);
  assert_eq!(account.index.query(&Query::default(), None, 100).unwrap().items[0]["title"], "saved");
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(
    &remote.calls.lock().unwrap()[calls..],
    &[
      (LibrarySource::FavoriteAlbums, 0),
      (LibrarySource::FavoriteTracks, 0),
      (LibrarySource::PurchasedAlbums, 0),
      (LibrarySource::PurchasedTracks, 0),
    ]
  );
  assert_eq!(
    account.index.query(&Query::default(), None, 100).unwrap().items[0]["title"],
    "Enriched record"
  );
  assert_eq!(progress.applied, account.mutation_versions());
  assert!(!progress.dirty);
}

#[tokio::test]
async fn transient_failure_resumes_the_last_page_without_restarting_the_snapshot() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  remote.enqueue(Ok(album("first", Some(1), 2)));
  remote.enqueue(Err(qobuz_api::ApiError::ServerError(503)));
  assert!(catalog.advance(&remote, &account, &mut progress, false).await.is_err());
  assert_eq!(progress.work.as_ref().unwrap().offset, 1);
  assert!(progress.retry_at.is_some());
  let calls = remote.calls.lock().unwrap().len();
  catalog.advance(&remote, &account, &mut progress, false).await.unwrap();
  assert_eq!(remote.calls.lock().unwrap().len(), calls, "Backoff must suppress requests");
  progress.retry_at = None;
  remote.enqueue(Ok(album("second", None, 2)));
  finish(&catalog, &remote, &account, &mut progress).await;
  let calls = remote.calls.lock().unwrap();
  assert_eq!(
    calls
      .iter()
      .filter(|(source, offset)| *source == LibrarySource::FavoriteAlbums && *offset == 0)
      .count(),
    1
  );
  assert_eq!(account.index.status().unwrap().items, 2);
}

#[tokio::test]
async fn known_mutation_reloads_only_its_source_even_when_service_revision_matches() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  finish(&catalog, &remote, &account, &mut progress).await;
  let before = remote.calls.lock().unwrap().len();
  remote.enqueue(Ok(album("added", None, 1)));
  account.changed(LibrarySource::FavoriteAlbums).unwrap();
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(&remote.calls.lock().unwrap()[before..], &[(LibrarySource::FavoriteAlbums, 0)]);
  assert_eq!(account.index.status().unwrap().items, 1);
  assert_eq!(progress.applied, account.mutation_versions());
  assert!(!progress.dirty);
}

#[tokio::test]
async fn a_known_mutation_before_opening_library_survives_restart_with_the_same_service_token() {
  let directory = tempfile::tempdir().unwrap();
  let session = SessionContext::new(directory.path().join("data"), directory.path().join("cache"));
  let remote = RemoteFixture::new();
  {
    let catalog = LibraryCatalog::default();
    let account = catalog.account(&session, 7).unwrap();
    let mut progress = account.progress.lock().await;
    finish(&catalog, &remote, &account, &mut progress).await;
  }
  let before = remote.calls.lock().unwrap().len();
  {
    let fresh_process = LibraryCatalog::default();
    let _mutation =
      fresh_process.changed_account(&session, 7, LibrarySource::FavoriteAlbums).await.unwrap();
  }
  let restarted = LibraryCatalog::default();
  let account = restarted.account(&session, 7).unwrap();
  assert_eq!(account.mutation_versions()[0], 1);
  let mut progress = account.progress.lock().await;
  assert_eq!(progress.applied[0], 0);
  remote.enqueue(Ok(album("added-before-restart", None, 1)));
  finish(&restarted, &remote, &account, &mut progress).await;
  assert_eq!(&remote.calls.lock().unwrap()[before..], &[(LibrarySource::FavoriteAlbums, 0)]);
  assert_eq!(
    account.index.query(&Query::default(), None, 100).unwrap().items[0]["ref"],
    "album:added-before-restart"
  );
  assert_eq!(
    account.index.local_versions().unwrap()[LibrarySource::FavoriteAlbums.name()].applied,
    1
  );
}

#[tokio::test]
async fn a_remote_mutation_holds_sync_without_creating_a_library_database() {
  let directory = tempfile::tempdir().unwrap();
  let session = SessionContext::new(directory.path().join("data"), directory.path().join("cache"));
  let catalog = LibraryCatalog::default();
  let remote = RemoteFixture::new();
  let mutation = catalog.changed_account(&session, 7, LibrarySource::FavoriteAlbums).await.unwrap();
  assert!(!LibraryIndex::exists(&session.cache, 7).unwrap());
  let account = catalog.account(&session, 7).unwrap();
  let mut progress = account.progress.lock().await;
  catalog.advance(&remote, &account, &mut progress, false).await.unwrap();
  assert!(remote.calls.lock().unwrap().is_empty());
  assert_eq!(account.index.status().unwrap().generation, 0);
  assert!(progress.dirty);
  drop(mutation);
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(remote.calls.lock().unwrap().len(), 8);
  assert!(account.index.status().unwrap().generation > 0);
}

#[tokio::test]
async fn a_failed_durable_mutation_mark_releases_the_gate_and_returns_an_error() {
  let directory = tempfile::tempdir().unwrap();
  let session = SessionContext::new(directory.path().join("data"), directory.path().join("cache"));
  let catalog = LibraryCatalog::default();
  let account = catalog.account(&session, 7).unwrap();
  let connection = rusqlite::Connection::open(session.cache.join("library-7.sqlite3")).unwrap();
  connection
    .execute_batch(
      "CREATE TRIGGER reject_local_change BEFORE INSERT ON local_versions
    BEGIN SELECT RAISE(ABORT,'fixture storage failure'); END;",
    )
    .unwrap();
  assert!(catalog.changed_account(&session, 7, LibrarySource::FavoriteAlbums).await.is_err());
  assert_eq!(account.mutation_versions(), [0; 8]);
  assert!(account.index.local_versions().unwrap().is_empty());
  assert!(catalog.mutations.try_read().is_ok());
}

#[tokio::test]
async fn mutation_during_sync_stays_pending_until_a_later_successful_publication() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  finish(&catalog, &remote, &account, &mut progress).await;
  account.changed(LibrarySource::FavoriteAlbums).unwrap();
  remote.enqueue(Ok(album("first", Some(1), 2)));
  remote.enqueue(Err(qobuz_api::ApiError::ServerError(503)));
  assert!(catalog.advance(&remote, &account, &mut progress, false).await.is_err());
  assert_eq!(progress.work.as_ref().unwrap().mutations[0], 1);
  assert_eq!(progress.applied[0], 0);
  account.changed(LibrarySource::FavoriteAlbums).unwrap();
  progress.retry_at = None;
  remote.enqueue(Ok(album("second", None, 2)));
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(progress.applied[0], 1);
  assert!(progress.dirty);
  assert!(LibraryCatalog::pending_mutations(&account, &progress));
  remote.enqueue(Ok(album("latest", None, 1)));
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(progress.applied[0], 2);
  assert!(!progress.dirty);
  let page = account.index.query(&Query::default(), None, 100).unwrap();
  assert_eq!(page.items.len(), 1);
  assert_eq!(page.items[0]["ref"], "album:latest");
}

#[tokio::test]
async fn a_snapshot_copy_resumes_from_its_cursor_before_fetching_changed_sources() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  remote.enqueue(Ok(SourcePage {
    items: (0..500)
      .map(|id| SourceRow {
        card: json!({"kind":"album","ref":format!("album:{id}"),"title":format!("Record {id}")}),
        added_at: 10,
        explicit: true,
      })
      .collect(),
    next_offset: Some(500),
    total: Some(501),
  }));
  remote.enqueue(Ok(album("last", None, 501)));
  finish(&catalog, &remote, &account, &mut progress).await;
  let before = account.index.status().unwrap();
  let calls = remote.calls.lock().unwrap().len();
  account.changed(LibrarySource::FollowedArtists).unwrap();
  let generation = account
    .index
    .begin_generation(&[Membership::Favorites, Membership::Purchases, Membership::Followed], 0)
    .unwrap();
  progress.work = Some(Sync {
    generation,
    source: 0,
    offset: 0,
    loaded: 0,
    source_unique: 0,
    source_total: None,
    page_size: SYNC_PAGE,
    changed: std::array::from_fn(|position| position == 2),
    revisions: remote.revisions.lock().unwrap().clone(),
    mutations: account.mutation_versions(),
    seed: Some(Seed { replaced: vec![(Membership::Followed, "artist".into())], cursor: None }),
  });
  assert!(!LibraryCatalog::seed_snapshot(&account, &mut progress, Instant::now(), 1).unwrap());
  assert!(progress.work.as_ref().unwrap().seed.as_ref().unwrap().cursor.is_some());
  assert_eq!(account.index.status().unwrap().generation, before.generation);
  assert_eq!(account.index.query(&Query::default(), None, 100).unwrap().items.len(), 100);
  assert_eq!(remote.calls.lock().unwrap().len(), calls);
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(&remote.calls.lock().unwrap()[calls..], &[(LibrarySource::FollowedArtists, 0)]);
  assert_eq!(account.index.status().unwrap().items, 501);
  assert_eq!(progress.applied[2], 1);
  assert!(!progress.dirty);
}

#[tokio::test]
async fn partially_repeated_pages_and_changing_totals_do_not_publish() {
  let page = |ids: &[&str], next_offset, total| SourcePage {
    items: ids
      .iter()
      .map(|id| SourceRow {
        card: json!({"kind":"album","ref":format!("album:{id}"),"title":id}),
        added_at: 10,
        explicit: true,
      })
      .collect(),
    next_offset,
    total,
  };
  for pages in [
    vec![page(&["a", "b"], Some(2), None), page(&["b", "c"], None, None)],
    vec![page(&["a"], Some(1), Some(3)), page(&["b"], None, Some(2))],
  ] {
    let directory = tempfile::tempdir().unwrap();
    let account = account(directory.path());
    let remote = RemoteFixture::new();
    let catalog = LibraryCatalog::default();
    let mut progress = Progress::default();
    for page in pages {
      remote.enqueue(Ok(page));
    }
    assert!(catalog.advance(&remote, &account, &mut progress, false).await.is_err());
    assert!(progress.work.is_none());
    assert_eq!(account.index.status().unwrap().generation, 0);
  }
}

#[tokio::test]
async fn a_slow_page_can_finish_after_the_soft_launch_budget() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let mut progress = Progress::default();
  *remote.delay_once.lock().unwrap() = Some(STEP_BUDGET + Duration::from_millis(50));
  remote.enqueue(Ok(album("first", Some(1), 2)));
  remote.enqueue(Ok(album("second", None, 2)));
  catalog.advance(&remote, &account, &mut progress, false).await.unwrap();
  assert_eq!(progress.work.as_ref().unwrap().offset, 1);
  assert!(progress.retry_at.is_none());
  finish(&catalog, &remote, &account, &mut progress).await;
  assert_eq!(account.index.status().unwrap().items, 2);
}

#[test]
fn visible_refresh_waits_for_backoff() {
  assert_eq!(LibraryCatalog::refresh_delay(true, Some(10000)), Some(10000));
  assert_eq!(LibraryCatalog::refresh_delay(true, None), Some(250));
  assert_eq!(LibraryCatalog::refresh_delay(false, Some(10000)), None);
}

#[tokio::test]
async fn failed_publication_can_retry_after_the_last_source_without_panicking() {
  let directory = tempfile::tempdir().unwrap();
  let account = account(directory.path());
  let remote = RemoteFixture::new();
  let catalog = LibraryCatalog::default();
  let generation = account.index.begin_generation(&[Membership::Favorites], 0).unwrap();
  let mut progress = Progress {
    work: Some(Sync {
      generation,
      source: 8,
      offset: 0,
      loaded: 0,
      source_unique: 0,
      source_total: None,
      page_size: 500,
      changed: [true; 8],
      revisions: remote.revisions.lock().unwrap().clone(),
      mutations: [0; 8],
      seed: None,
    }),
    ..Default::default()
  };
  assert!(catalog.advance(&remote, &account, &mut progress, false).await.is_err());
  assert!(progress.work.is_none());
  assert!(progress.planned.is_none());
  assert!(progress.dirty);
  assert!(progress.last_error.is_some());
  catalog.advance(&remote, &account, &mut progress, false).await.unwrap();
  assert!(remote.calls.lock().unwrap().is_empty());
  assert_eq!(account.index.status().unwrap().generation, 0);
  progress.retry_at = None;
  finish(&catalog, &remote, &account, &mut progress).await;
  assert!(account.index.status().unwrap().generation > 0);
  assert!(progress.last_error.is_none());
}
#[test]
fn filters_and_cursor_survive_opaque_navigation() {
  let request = json!({"ref":"library:all","filters":{"favorite":true,"hide_purchases":true,"genres":["1"]},"order":"title"});
  let mut selection = Selection::read(&request).unwrap();
  selection.cursor = Some("fixture-cursor".into());
  let restored = Selection::read(&json!({"ref":selection.reference().unwrap()})).unwrap();
  assert_eq!(restored.cursor.as_deref(), Some("fixture-cursor"));
  assert!(restored.filters.favorite && restored.filters.hide_purchases);
  assert_eq!(restored.order, Order::Title);
  assert_eq!(restored.query(7).unwrap().membership_groups, vec![vec![Membership::Favorites]]);
}
#[test]
fn invalid_views_and_conflicting_filters_are_rejected() {
  for request in [
    json!({"ref":"library:unknown"}),
    json!({"ref":"library:all","filters":{"purchased":true,"hide_purchases":true}}),
    json!({"ref":"library:all","filters":{"membership":"purchases","hide_purchases":true}}),
    json!({"ref":"library:all","filters":{"membership":"unknown"}}),
    json!({"ref":"library:v1:invalid"}),
  ] {
    assert!(Selection::read(&request).is_err());
  }
}

#[test]
fn collection_choices_replace_stale_legacy_fields_and_preserve_opaque_navigation() {
  let selection = Selection::read(&json!({"ref":"library:albums","filters":{
    "favorite":true,"genres":["old"],"membership":"purchases","genre":"42"}}))
  .unwrap();
  let query = selection.query(7).unwrap();
  assert_eq!(query.membership_groups, vec![vec![Membership::Purchases]]);
  assert_eq!(query.genres, vec!["42"]);
  assert!(!selection.filters.favorite);
  let restored = Selection::read(&json!({"ref":selection.reference().unwrap()})).unwrap();
  assert_eq!(restored.filters.presented()["membership"], "purchases");
  assert_eq!(restored.filters.presented()["genre"], "42");
  let cleared = Selection::read(&json!({"ref":"library:all","filters":{
    "favorite":true,"purchased":true,"membership":"all","genres":["old"],"genre":""}}))
  .unwrap();
  assert!(cleared.query(7).unwrap().membership_groups.is_empty());
  assert!(cleared.query(7).unwrap().genres.is_empty());
}

#[test]
fn older_filter_requests_keep_intersection_and_multiple_genres() {
  let selection = Selection::read(&json!({"ref":"library:all","filters":{
    "favorite":true,"purchased":true,"genres":["1","2"]}}))
  .unwrap();
  let query = selection.query(7).unwrap();
  assert_eq!(
    query.membership_groups,
    vec![vec![Membership::Favorites], vec![Membership::Purchases]]
  );
  assert_eq!(query.genres, vec!["1", "2"]);
}

#[test]
fn awards_default_to_catalog_title_order_and_expose_following_filter() {
  let selection = Selection::read(&json!({"ref":"library:awards"})).unwrap();
  assert_eq!(selection.order, Order::Title);
  let controls = crate::library_sources::filter_controls("awards", false, false, &[]);
  let membership = controls.iter().find(|control| control["key"] == "membership").unwrap();
  assert_eq!(membership["type"], "choice");
  assert_eq!(membership["options"][0]["value"], "all");
  assert!(membership["options"]
    .as_array()
    .unwrap()
    .iter()
    .any(|option| option["value"] == "followed"));
}
