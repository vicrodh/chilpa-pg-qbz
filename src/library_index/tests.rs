use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::json;
use std::time::Instant;

fn entity(kind: &str, id: u64, title: &str) -> Value {
  json!({"kind":kind,"ref":format!("{kind}:{id:012}"),"title":title,"genres":["Rock"],"owner_id":"11","quality":{"codec":"flac","bits":24,"rate":96000}})
}

fn ingest(
  index: &LibraryIndex,
  generation: Generation,
  membership: Membership,
  item: &Value,
  added_at: i64,
  explicit: bool,
) {
  index.ingest(generation, membership, &[IndexedItem { item, added_at, explicit }]).unwrap();
}

fn all() -> Query {
  Query { explicit_only: true, ..Default::default() }
}

fn refs(page: &Page) -> Vec<String> {
  page.items.iter().map(|item| item["ref"].as_str().unwrap().to_owned()).collect()
}

fn publish(index: &LibraryIndex, collections: &[Membership]) -> Generation {
  let generation = index.begin_generation(collections, 1).unwrap();
  for collection in collections {
    index.finish_collection(generation, *collection).unwrap();
  }
  index.publish(generation, 2).unwrap();
  generation
}

#[test]
fn duplicate_collections_preserve_flags_dates_and_explicit_membership() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation =
    index.begin_generation(&[Membership::Favorites, Membership::Purchases], 1).unwrap();
  let first = entity("album", 1, "Shared");
  ingest(&index, generation, Membership::Favorites, &first, 30, true);
  let mut purchase = first.clone();
  purchase["favorite"] = false.into();
  ingest(&index, generation, Membership::Purchases, &purchase, 20, true);
  let implicit = entity("artist", 2, "Inferred artist");
  ingest(&index, generation, Membership::Favorites, &implicit, 40, false);
  index.finish_collection(generation, Membership::Favorites).unwrap();
  index.finish_collection(generation, Membership::Purchases).unwrap();
  let status = index.publish(generation, 100).unwrap();
  assert_eq!(status.items, 2);
  let page = index.query(&all(), None, 100).unwrap();
  assert_eq!(page.items.len(), 1);
  assert_eq!(page.items[0]["favorite"], true);
  assert_eq!(page.items[0]["purchased"], true);
  assert_eq!(page.items[0]["added_at"], 30);
  assert_eq!(page.items[0]["collections"].as_array().unwrap().len(), 2);
  assert_eq!(index.query(&Query::default(), None, 100).unwrap().items.len(), 2);
}

#[test]
fn repeated_pages_report_no_new_memberships_and_another_collection_keeps_entity_identity() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation =
    index.begin_generation(&[Membership::Favorites, Membership::Purchases], 1).unwrap();
  let first = entity("album", 1, "First");
  let second = entity("track", 2, "Second");
  let chunk = [
    IndexedItem { item: &first, added_at: 1, explicit: true },
    IndexedItem { item: &second, added_at: 2, explicit: true },
  ];
  assert_eq!(
    index.ingest(generation, Membership::Favorites, &chunk).unwrap(),
    IngestStats { new_entities: 2, new_memberships: 2 }
  );
  assert_eq!(
    index.ingest(generation, Membership::Favorites, &chunk).unwrap(),
    IngestStats::default()
  );
  assert_eq!(
    index.ingest(generation, Membership::Purchases, &chunk[..1]).unwrap(),
    IngestStats { new_entities: 0, new_memberships: 1 }
  );
}

#[test]
fn equal_refresh_keeps_cursors_and_revision_while_membership_date_changes_publish() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let collections = [Membership::Favorites, Membership::Purchases];
  let one = entity("album", 1, "One");
  let two = entity("track", 2, "Two");
  let first = index.begin_generation(&collections, 1).unwrap();
  ingest(&index, first, Membership::Favorites, &one, 30, true);
  ingest(&index, first, Membership::Favorites, &two, 20, true);
  let mut purchased = one.clone();
  purchased["favorite"] = false.into();
  ingest(&index, first, Membership::Purchases, &purchased, 10, true);
  for collection in collections {
    index.finish_collection(first, collection).unwrap();
  }
  let original = index.publish(first, 40).unwrap();
  let page = index.query(&all(), None, 1).unwrap();
  let repeated = index.begin_generation(&collections, 41).unwrap();
  ingest(&index, repeated, Membership::Purchases, &purchased, 10, true);
  ingest(&index, repeated, Membership::Favorites, &two, 20, true);
  ingest(&index, repeated, Membership::Favorites, &one, 30, true);
  for collection in collections {
    index.finish_collection(repeated, collection).unwrap();
  }
  let unchanged = index.publish(repeated, 50).unwrap();
  assert_eq!(unchanged.generation, original.generation);
  assert_eq!(unchanged.revision, original.revision);
  assert_eq!(unchanged.checked_at, 50);
  assert_eq!(
    refs(&index.query(&all(), page.next.as_deref(), 1).unwrap()),
    vec![two["ref"].as_str().unwrap()]
  );
  assert!(!index.prune_chunk(500).unwrap().pending);
  let changed = index.begin_generation(&collections, 51).unwrap();
  ingest(&index, changed, Membership::Favorites, &one, 30, true);
  ingest(&index, changed, Membership::Favorites, &two, 20, true);
  ingest(&index, changed, Membership::Purchases, &purchased, 11, true);
  for collection in collections {
    index.finish_collection(changed, collection).unwrap();
  }
  let published = index.publish(changed, 60).unwrap();
  assert_ne!(published.generation, original.generation);
  assert_eq!(published.revision, original.revision + 1);
  assert!(index.query(&all(), page.next.as_deref(), 1).is_err());
}

#[test]
fn playlist_owner_zero_genre_ids_and_advertised_hires_remain_filterable() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  let generation = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  for (id, owner) in [(1, 0), (2, 7), (3, 22)] {
    let mut item = entity("playlist", id, "Playlist");
    item["owner_id"] = owner.into();
    item["quality"] = Value::Null;
    item["hires"] = true.into();
    item["genre_ids"] = json!([112]);
    ingest(&index, generation, Membership::Favorites, &item, 1, true);
  }
  ingest(&index, generation, Membership::Favorites, &entity("artist", 4, "Artist"), 2, true);
  index.finish_collection(generation, Membership::Favorites).unwrap();
  index.publish(generation, 3).unwrap();
  let query = Query {
    kinds: vec!["playlist".into()],
    owners: vec!["0".into()],
    genres: vec!["112".into()],
    hires: Some(true),
    ..all()
  };
  assert_eq!(refs(&index.query(&query, None, 100).unwrap()), vec!["playlist:000000000001"]);
  let others =
    Query { kinds: vec!["playlist".into()], exclude_owners: vec!["0".into(), "7".into()], ..all() };
  assert_eq!(refs(&index.query(&others, None, 100).unwrap()), vec!["playlist:000000000003"]);
}

#[test]
fn filters_intersect_groups_and_union_their_selected_values() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let collections = [Membership::Favorites, Membership::Purchases, Membership::Followed];
  let generation = index.begin_generation(&collections, 1).unwrap();
  let album = entity("album", 1, "Shared record");
  let mut track = entity("track", 2, "Other song");
  track["genres"] = json!(["Jazz"]);
  track["owner_id"] = "22".into();
  let artist = entity("artist", 3, "Followed artist");
  ingest(&index, generation, Membership::Favorites, &album, 3, true);
  ingest(&index, generation, Membership::Purchases, &album, 2, true);
  ingest(&index, generation, Membership::Favorites, &track, 4, true);
  ingest(&index, generation, Membership::Followed, &artist, 5, true);
  for collection in collections {
    index.finish_collection(generation, collection).unwrap();
  }
  index.publish(generation, 6).unwrap();
  let mut query = Query {
    kinds: vec!["album".into(), "track".into()],
    genres: vec!["rock".into(), "jazz".into()],
    membership_groups: vec![vec![Membership::Favorites, Membership::Followed]],
    hires: Some(true),
    ..all()
  };
  assert_eq!(index.query(&query, None, 100).unwrap().items.len(), 2);
  query.owners = vec!["11".into()];
  query.membership_groups.push(vec![Membership::Purchases]);
  assert_eq!(refs(&index.query(&query, None, 100).unwrap()), vec![album["ref"].as_str().unwrap()]);
  query.exclude_purchases = true;
  assert!(index.query(&query, None, 100).unwrap().items.is_empty());
  query.membership_groups = vec![vec![Membership::Favorites]];
  query.owners.clear();
  assert_eq!(refs(&index.query(&query, None, 100).unwrap()), vec![track["ref"].as_str().unwrap()]);
  query.hires = Some(false);
  assert!(index.query(&query, None, 100).unwrap().items.is_empty());
}

#[test]
fn keyset_pages_keep_ties_and_reject_another_query_account_or_generation() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  for (kind, id, title) in
    [("track", 4, "Beta"), ("album", 2, "Alpha"), ("artist", 3, "Alpha"), ("album", 1, "Alpha")]
  {
    ingest(&index, generation, Membership::Favorites, &entity(kind, id, title), 7, true);
  }
  index.finish_collection(generation, Membership::Favorites).unwrap();
  index.publish(generation, 2).unwrap();
  let first = index.query(&all(), None, 2).unwrap();
  let cursor = first.next.as_deref().unwrap();
  let second = index.query(&all(), Some(cursor), 2).unwrap();
  let mut actual = refs(&first);
  actual.extend(refs(&second));
  assert_eq!(
    actual,
    vec!["album:000000000001", "album:000000000002", "artist:000000000003", "track:000000000004"]
  );
  assert!(!second.has_more);
  let changed = Query { exclude_purchases: true, ..all() };
  assert!(index.query(&changed, Some(cursor), 2).is_err());
  let another = LibraryIndex::open(directory.path(), 2).unwrap();
  publish(&another, &[Membership::Favorites]);
  assert!(another.query(&all(), Some(cursor), 2).is_err());
  let titled = Query { order: Order::Title, ..all() };
  let title_first = index.query(&titled, None, 2).unwrap();
  let title_next = index.query(&titled, title_first.next.as_deref(), 2).unwrap();
  let mut titles = refs(&title_first);
  titles.extend(refs(&title_next));
  assert_eq!(titles, actual);
  publish(&index, &[Membership::Favorites]);
  assert!(index.query(&all(), Some(cursor), 2).unwrap_err().contains("stale"));
}

#[test]
fn incomplete_failed_and_restarted_syncs_leave_the_published_snapshot_usable() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let first = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  ingest(&index, first, Membership::Favorites, &entity("album", 1, "Original"), 1, true);
  index.finish_collection(first, Membership::Favorites).unwrap();
  index.publish(first, 2).unwrap();
  let original = refs(&index.query(&all(), None, 100).unwrap());
  let incomplete =
    index.begin_generation(&[Membership::Favorites, Membership::Purchases], 3).unwrap();
  ingest(&index, incomplete, Membership::Favorites, &entity("album", 2, "Partial"), 2, true);
  index.finish_collection(incomplete, Membership::Favorites).unwrap();
  assert!(index.publish(incomplete, 4).is_err());
  assert_eq!(refs(&index.query(&all(), None, 100).unwrap()), original);
  index.abandon(incomplete).unwrap();
  assert!(!index.prune_chunk(500).unwrap().pending);
  let failed = index.begin_generation(&[Membership::Favorites], 5).unwrap();
  let valid = entity("album", 3, "Must roll back");
  let invalid = json!({"kind":"unsupported","ref":"wrong:1"});
  assert!(index
    .ingest(
      failed,
      Membership::Favorites,
      &[
        IndexedItem { item: &valid, added_at: 3, explicit: true },
        IndexedItem { item: &invalid, added_at: 3, explicit: true }
      ]
    )
    .is_err());
  assert!(index.finish_collection(failed, Membership::Favorites).is_err());
  assert!(index.publish(failed, 6).is_err());
  assert_eq!(refs(&index.query(&all(), None, 100).unwrap()), original);
  assert!(!index.prune_chunk(500).unwrap().pending);
  let interrupted = index.begin_generation(&[Membership::Favorites], 7).unwrap();
  ingest(&index, interrupted, Membership::Favorites, &entity("album", 4, "Interrupted"), 4, true);
  drop(index);
  let reopened = LibraryIndex::open(directory.path(), 1).unwrap();
  assert!(reopened.publish(interrupted, 8).is_err());
  assert_eq!(refs(&reopened.query(&all(), None, 100).unwrap()), original);
}

#[test]
fn payload_and_sqlite_capacity_failures_cannot_publish_partial_data() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  publish(&index, &[Membership::Favorites]);
  let oversized = index.begin_generation(&[Membership::Favorites], 3).unwrap();
  let mut item = entity("album", 1, "Oversized");
  item["description"] = "x".repeat(MAX_PAYLOAD).into();
  assert!(index
    .ingest(
      oversized,
      Membership::Favorites,
      &[IndexedItem { item: &item, added_at: 1, explicit: true }]
    )
    .is_err());
  assert!(index.publish(oversized, 4).is_err());
  index.prune_chunk(500).unwrap();
  let limited = index.begin_generation(&[Membership::Favorites], 5).unwrap();
  {
    let connection = index.lock();
    let pages: i64 = connection.pragma_query_value(None, "page_count", |row| row.get(0)).unwrap();
    connection.pragma_update(None, "max_page_count", pages + 1).unwrap();
  }
  item["description"] = "x".repeat(60 * 1024).into();
  assert!(index
    .ingest(
      limited,
      Membership::Favorites,
      &[IndexedItem { item: &item, added_at: 1, explicit: true }]
    )
    .is_err());
  assert!(index.publish(limited, 6).is_err());
  assert_eq!(index.status().unwrap().generation, 1);
  assert!(index.query(&all(), None, 100).unwrap().items.is_empty());
}

#[test]
fn page_and_cleanup_work_is_bounded_and_default_orders_seek_indexes() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  seed(&index, 1200);
  let first = index.query(&all(), None, usize::MAX).unwrap();
  assert_eq!(first.items.len(), 100);
  let query_plan = index.explain(&all(), first.next.as_deref(), 100).unwrap();
  assert!(
    query_plan.iter().any(
      |line| line.contains("SEARCH e USING INDEX entities_added") && line.contains("added_key")
    ),
    "{query_plan:?}"
  );
  assert!(!query_plan.iter().any(|line| line.contains("TEMP B-TREE")), "{query_plan:?}");
  let titled = index.explain(&Query { order: Order::Title, ..all() }, None, 100).unwrap();
  assert!(
    titled.iter().any(|line| line.contains("SEARCH e USING INDEX entities_title")),
    "{titled:?}"
  );
  publish(&index, &[Membership::Favorites]);
  assert!(index.begin_generation(&[Membership::Favorites], 3).is_err());
  let progress = index.prune_chunk(usize::MAX).unwrap();
  assert_eq!(progress.removed, 500);
  assert!(progress.pending);
  assert!(index.query(&all(), None, 100).unwrap().items.is_empty());
  assert_eq!(index.prune_chunk(500).unwrap().removed, 500);
  assert!(!index.prune_chunk(500).unwrap().pending);
  assert!(index.begin_generation(&[Membership::Favorites], 4).is_ok());
}

#[test]
fn partial_queries_default_safely_and_text_metacharacters_are_literal() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  ingest(&index, generation, Membership::Favorites, &entity("album", 1, "100%_Complete"), 1, true);
  ingest(&index, generation, Membership::Favorites, &entity("album", 2, "100xxComplete"), 2, true);
  index.finish_collection(generation, Membership::Favorites).unwrap();
  index.publish(generation, 2).unwrap();
  let query: Query = serde_json::from_value(json!({"text":"%_","explicit_only":true})).unwrap();
  assert_eq!(index.query(&query, None, 100).unwrap().items.len(), 1);
  assert!(index.query(&Query { text: "x".repeat(257), ..all() }, None, 100).is_err());
  let connection = index.lock();
  assert_eq!(
    connection.pragma_query_value(None, "cache_size", |row| row.get::<_, i64>(0)).unwrap(),
    -4096
  );
  assert_eq!(
    connection.pragma_query_value(None, "mmap_size", |row| row.get::<_, i64>(0)).unwrap(),
    0
  );
}

#[test]
fn absent_genre_uses_the_genre_index_and_still_validates_its_cursor() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  seed(&index, 1200);
  let query = Query { genres: vec!["missing-fixture-genre".into()], ..all() };
  let page = index.query(&query, None, 100).unwrap();
  assert!(page.items.is_empty() && page.next.is_none() && !page.has_more);
  assert_eq!(page.status.items, 1200);
  assert!(index.query(&query, Some("invalid-cursor"), 100).is_err());
  let plan = index.explain_genre_presence(&query.genres).unwrap();
  assert!(
    plan
      .iter()
      .any(|line| line.contains("genres_lookup") && line.contains("generation=? AND genre=?")),
    "{plan:?}"
  );
  assert!(!plan.iter().any(|line| line.contains("SCAN entity_genres")), "{plan:?}");
}

fn fixture(id: u64) -> Value {
  let kind = ["album", "track", "artist", "label", "award", "playlist"][(id % 6) as usize];
  let mut item = entity(kind, id, &format!("Entity {id:012}"));
  item["genres"] = json!([if id % 3 == 0 { "Jazz" } else { "Rock" }]);
  item
}

fn stage(index: &LibraryIndex, count: u64) -> Generation {
  let generation =
    index.begin_generation(&[Membership::Favorites, Membership::Purchases], 1).unwrap();
  for begin in (0..count).step_by(MAX_CHUNK) {
    let values: Vec<_> = (begin..(begin + MAX_CHUNK as u64).min(count)).map(fixture).collect();
    let chunk: Vec<_> = values
      .iter()
      .enumerate()
      .map(|(offset, item)| IndexedItem {
        item,
        added_at: ((begin + offset as u64) / 3 + 1) as i64,
        explicit: true,
      })
      .collect();
    index.ingest(generation, Membership::Favorites, &chunk).unwrap();
    let purchases: Vec<_> = chunk
      .into_iter()
      .enumerate()
      .filter_map(|(offset, item)| ((begin + offset as u64) % 4 == 0).then_some(item))
      .collect();
    index.ingest(generation, Membership::Purchases, &purchases).unwrap();
  }
  index.finish_collection(generation, Membership::Favorites).unwrap();
  index.finish_collection(generation, Membership::Purchases).unwrap();
  generation
}

fn seed(index: &LibraryIndex, count: u64) {
  let generation = stage(index, count);
  index.publish(generation, 2).unwrap();
}

#[test]
#[ignore = "Run with LIBRARY_INDEX_BENCH_ROWS=100000 or 1000000 and measure process RSS externally"]
fn benchmark_large_library_index() {
  let count: u64 = std::env::var("LIBRARY_INDEX_BENCH_ROWS")
    .ok()
    .and_then(|value| value.parse().ok())
    .unwrap_or(100000);
  assert!(matches!(count, 100000 | 1000000));
  let directory = tempfile::tempdir().unwrap();
  let started = Instant::now();
  let index = LibraryIndex::open(directory.path(), 999).unwrap();
  seed(&index, count);
  drop(index);
  let sync_ms = started.elapsed().as_secs_f64() * 1000.0;
  let opened = Instant::now();
  let index = LibraryIndex::open(directory.path(), 999).unwrap();
  let reopen_ms = opened.elapsed().as_secs_f64() * 1000.0;
  let first_start = Instant::now();
  let first = index.query(&all(), None, 100).unwrap();
  let first_ms = first_start.elapsed().as_secs_f64() * 1000.0;
  assert_eq!(first.status.items, count);
  // The generated fixture supplies a valid late key without walking earlier pages or using OFFSET.
  let anchor = count / 2;
  let mut encoded: Value =
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(first.next.as_deref().unwrap()).unwrap())
      .unwrap();
  let anchor_item = fixture(anchor);
  encoded["key"] = (-((anchor / 3 + 1) as i64)).into();
  encoded["kind"] = anchor_item["kind"].clone();
  encoded["reference"] = anchor_item["ref"].clone();
  let late_cursor = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&encoded).unwrap());
  let late_start = Instant::now();
  let late = index.query(&all(), Some(&late_cursor), 100).unwrap();
  let late_ms = late_start.elapsed().as_secs_f64() * 1000.0;
  assert_eq!(late.items.len(), 100);
  let filtered = Query {
    genres: vec!["jazz".into()],
    membership_groups: vec![vec![Membership::Purchases]],
    ..all()
  };
  let filter_start = Instant::now();
  let result = index.query(&filtered, None, 100).unwrap();
  let filter_ms = filter_start.elapsed().as_secs_f64() * 1000.0;
  assert_eq!(result.items.len(), 100);
  let absent = Query {
    genres: vec!["absent-fixture-genre".into()],
    membership_groups: vec![vec![Membership::Purchases]],
    ..all()
  };
  let absent_start = Instant::now();
  let absent_page = index.query(&absent, None, 100).unwrap();
  let absent_ms = absent_start.elapsed().as_secs_f64() * 1000.0;
  assert!(absent_page.items.is_empty() && !absent_page.has_more);
  let first_plan = index.explain(&all(), None, 100).unwrap();
  let late_plan = index.explain(&all(), Some(&late_cursor), 100).unwrap();
  let filtered_plan = index.explain(&filtered, None, 100).unwrap();
  let absent_plan = index.explain(&absent, None, 100).unwrap();
  let absent_preflight_plan = index.explain_genre_presence(&absent.genres).unwrap();
  let staged = stage(&index, count);
  let gate = std::sync::Barrier::new(2);
  let done = std::sync::atomic::AtomicBool::new(false);
  let (equal, compare_ms, concurrent_first_ms, concurrent_late_ms, overlapped) =
    std::thread::scope(|scope| {
      let publishing = scope.spawn(|| {
        gate.wait();
        let started = Instant::now();
        let status = index.publish(staged, 3).unwrap();
        done.store(true, std::sync::atomic::Ordering::Release);
        (status, started.elapsed().as_secs_f64() * 1000.0)
      });
      gate.wait();
      let started = Instant::now();
      assert_eq!(index.query(&all(), None, 100).unwrap().items.len(), 100);
      let first_ms = started.elapsed().as_secs_f64() * 1000.0;
      let started = Instant::now();
      assert_eq!(index.query(&all(), Some(&late_cursor), 100).unwrap().items.len(), 100);
      let late_ms = started.elapsed().as_secs_f64() * 1000.0;
      let overlapped = !done.load(std::sync::atomic::Ordering::Acquire);
      let (status, compare_ms) = publishing.join().unwrap();
      (status, compare_ms, first_ms, late_ms, overlapped)
    });
  assert_eq!(equal.generation, first.status.generation);
  assert_eq!(equal.revision, first.status.revision);
  assert!(index.query(&all(), first.next.as_deref(), 100).is_ok());
  let cleanup_started = Instant::now();
  let mut cleanup_chunks = 0_u64;
  loop {
    let progress = index.prune_chunk(500).unwrap();
    assert!(progress.removed <= 500);
    cleanup_chunks += 1;
    if !progress.pending {
      break;
    }
  }
  let cleanup_ms = cleanup_started.elapsed().as_secs_f64() * 1000.0;
  let incremental =
    index.begin_generation(&[Membership::Favorites, Membership::Purchases], 4).unwrap();
  let seed_plans = index
    .explain_seed_chunk(
      incremental,
      &[(Membership::Favorites, "album".into())],
      ("album", "album:000000008000"),
      ("album", "album:000000008500"),
    )
    .unwrap();
  let seed_gate = std::sync::Barrier::new(2);
  let seeded = std::sync::atomic::AtomicBool::new(false);
  let (
    copied,
    incremental_seed_ms,
    seed_max_chunk_ms,
    seed_chunks,
    seed_first_ms,
    seed_late_ms,
    seed_overlapped,
  ) = std::thread::scope(|scope| {
    let seeding = scope.spawn(|| {
      seed_gate.wait();
      let started = Instant::now();
      let mut copied = IngestStats::default();
      let mut cursor = None;
      let mut max_chunk_ms = 0.0_f64;
      let mut chunks = 0_u64;
      loop {
        let chunk_started = Instant::now();
        let progress = index
          .seed_from_current_chunk(
            incremental,
            &[(Membership::Favorites, "album".into())],
            cursor.as_deref(),
            MAX_CHUNK,
          )
          .unwrap();
        max_chunk_ms = max_chunk_ms.max(chunk_started.elapsed().as_secs_f64() * 1000.0);
        chunks += 1;
        copied.new_entities += progress.stats.new_entities;
        copied.new_memberships += progress.stats.new_memberships;
        if progress.done {
          break;
        }
        cursor = progress.next;
      }
      seeded.store(true, std::sync::atomic::Ordering::Release);
      (copied, started.elapsed().as_secs_f64() * 1000.0, max_chunk_ms, chunks)
    });
    seed_gate.wait();
    let started = Instant::now();
    let during = index.query(&all(), None, 100).unwrap();
    assert_eq!(during.status.generation, first.status.generation);
    assert_eq!(during.status.items, count);
    assert_eq!(during.items.len(), 100);
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    assert_eq!(index.query(&all(), Some(&late_cursor), 100).unwrap().items.len(), 100);
    let late_ms = started.elapsed().as_secs_f64() * 1000.0;
    let overlapped = !seeded.load(std::sync::atomic::Ordering::Acquire);
    let (copied, seed_ms, max_chunk_ms, chunks) = seeding.join().unwrap();
    (copied, seed_ms, max_chunk_ms, chunks, first_ms, late_ms, overlapped)
  });
  let album_count = count.div_ceil(6);
  let favorite_only_albums = (count + 5) / 12;
  assert_eq!(copied.new_entities, count - favorite_only_albums);
  assert_eq!(copied.new_memberships, count + count.div_ceil(4) - album_count);
  let ingest_started = Instant::now();
  for begin in (0..album_count).step_by(MAX_CHUNK) {
    let values: Vec<_> = (begin..(begin + MAX_CHUNK as u64).min(album_count))
      .map(|position| {
        let mut item = fixture(position * 6);
        if position == 0 {
          item["title"] = "Revised entity 000000000000".into();
        }
        item
      })
      .collect();
    let chunk: Vec<_> = values
      .iter()
      .enumerate()
      .map(|(offset, item)| IndexedItem {
        item,
        added_at: (((begin + offset as u64) * 6) / 3 + 1) as i64,
        explicit: true,
      })
      .collect();
    let stats = index.ingest(incremental, Membership::Favorites, &chunk).unwrap();
    assert_eq!(stats.new_memberships, chunk.len() as u64);
  }
  let incremental_ingest_ms = ingest_started.elapsed().as_secs_f64() * 1000.0;
  index.finish_collection(incremental, Membership::Favorites).unwrap();
  index.finish_collection(incremental, Membership::Purchases).unwrap();
  let publish_started = Instant::now();
  let changed = index.publish(incremental, 5).unwrap();
  let incremental_publish_ms = publish_started.elapsed().as_secs_f64() * 1000.0;
  assert_eq!(changed.items, count);
  assert_ne!(changed.generation, first.status.generation);
  assert!(index.query(&all(), Some(&late_cursor), 100).is_err());
  assert_eq!(index.query(&all(), None, 100).unwrap().items.len(), 100);
  println!(
    "{}",
    json!({"rows":count,"sync_ms":sync_ms,"reopen_ms":reopen_ms,"first_page_ms":first_ms,"late_page_ms":late_ms,"filtered_page_ms":filter_ms,"equal_refresh_compare_ms":compare_ms,
    "concurrent_first_page_ms":concurrent_first_ms,"concurrent_late_page_ms":concurrent_late_ms,"comparison_overlapped_queries":overlapped,
    "absent_filter_page_ms":absent_ms,"cleanup_ms":cleanup_ms,"cleanup_chunks":cleanup_chunks,
    "incremental_seed_ms":incremental_seed_ms,"incremental_seed_entities":copied.new_entities,"incremental_seed_memberships":copied.new_memberships,
    "seed_max_chunk_ms":seed_max_chunk_ms,"seed_chunks":seed_chunks,"seed_plans":seed_plans,
    "incremental_ingest_ms":incremental_ingest_ms,"incremental_publish_ms":incremental_publish_ms,
    "seed_concurrent_first_page_ms":seed_first_ms,"seed_concurrent_late_page_ms":seed_late_ms,"seed_overlapped_queries":seed_overlapped,
    "database_bytes":std::fs::metadata(directory.path().join("library-999.sqlite3")).unwrap().len(),
    "first_plan":first_plan,"late_plan":late_plan,"filtered_plan":filtered_plan,"absent_plan":absent_plan,"absent_preflight_plan":absent_preflight_plan})
  );
}
