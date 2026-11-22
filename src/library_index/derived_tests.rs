use super::*;
use rusqlite::params;
use serde_json::json;

const COLLECTIONS: [Membership; 3] =
  [Membership::Favorites, Membership::Purchases, Membership::Followed];

fn parent(id: u64, artist: &str) -> Value {
  json!({"kind":"album","ref":format!("album:{id}"),"title":format!("Record {id}"),
    "image":"https://images.invalid/album.jpg","artist_ref":"artist:1","artist":artist,
    "label_ref":"label:1","label":"Label"})
}

fn add(
  index: &LibraryIndex,
  generation: Generation,
  collection: Membership,
  item: &Value,
  date: i64,
) {
  index
    .ingest(generation, collection, &[IndexedItem { item, added_at: date, explicit: true }])
    .unwrap();
}

fn publish(index: &LibraryIndex, generation: Generation) -> IndexStatus {
  for collection in COLLECTIONS {
    index.finish_collection(generation, collection).unwrap();
  }
  index.publish(generation, 10).unwrap()
}

fn rows(index: &LibraryIndex, kind: &str, explicit: bool) -> Vec<Value> {
  index
    .query(
      &Query { kinds: vec![kind.into()], explicit_only: explicit, ..Default::default() },
      None,
      100,
    )
    .unwrap()
    .items
}

fn clean(index: &LibraryIndex) {
  while index.prune_chunk(500).unwrap().pending {}
}

#[test]
fn inferred_entities_rebuild_from_surviving_parents_and_disappear_after_the_last_support() {
  let directory = tempfile::tempdir().unwrap();
  let full_directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let full = LibraryIndex::open(full_directory.path(), 1).unwrap();
  let one = parent(1, "First artist");
  let two = parent(2, "Second artist");
  let first = index.begin_generation(&COLLECTIONS, 1).unwrap();
  add(&index, first, Membership::Favorites, &two, 40);
  add(&index, first, Membership::Favorites, &one, 30);
  add(&index, first, Membership::Purchases, &one, 20);
  publish(&index, first);
  let artists = rows(&index, "artist", false);
  assert_eq!(artists.len(), 1);
  assert_eq!(artists[0]["title"], "First artist");
  assert!(artists[0].get("image").is_none(), "An inferred artist never inherits the album cover");
  assert_eq!(artists[0]["favorite"], false);
  assert_eq!(artists[0]["purchased"], false);
  assert_eq!(artists[0]["library_explicit"], false);
  assert!(rows(&index, "artist", true).is_empty());
  assert!(rows(&index, "label", true).is_empty());

  let retained = index.begin_generation(&COLLECTIONS, 2).unwrap();
  index.seed_from_current(retained, &[(Membership::Favorites, "album".into())]).unwrap();
  publish(&index, retained);
  let complete = full.begin_generation(&COLLECTIONS, 2).unwrap();
  add(&full, complete, Membership::Purchases, &one, 20);
  publish(&full, complete);
  assert_eq!(
    index.query(&Query::default(), None, 100).unwrap().items,
    full.query(&Query::default(), None, 100).unwrap().items
  );
  assert_eq!(rows(&index, "artist", false).len(), 1, "The purchase still supports the artist");
  assert_eq!(rows(&index, "label", false).len(), 1);

  clean(&index);
  let removed = index.begin_generation(&COLLECTIONS, 3).unwrap();
  index.seed_from_current(removed, &[(Membership::Purchases, "album".into())]).unwrap();
  publish(&index, removed);
  assert!(index.query(&Query::default(), None, 100).unwrap().items.is_empty());
}

#[test]
fn explicit_following_controls_all_dates_flags_and_filters_without_derived_membership_leaks() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let first = index.begin_generation(&COLLECTIONS, 1).unwrap();
  let followed = json!({"kind":"artist","ref":"artist:1","title":"Followed name","image":"https://images.invalid/artist.jpg"});
  add(&index, first, Membership::Followed, &followed, 10);
  add(&index, first, Membership::Favorites, &parent(1, "Derived favorite"), 100);
  add(&index, first, Membership::Purchases, &parent(2, "Derived purchase"), 200);
  publish(&index, first);
  let artist = rows(&index, "artist", true).pop().unwrap();
  assert_eq!(artist["title"], "Followed name");
  assert_eq!(artist["added_at"], 10);
  assert_eq!(artist["favorite"], true);
  assert_eq!(artist["purchased"], false);
  assert_eq!(artist["image"], followed["image"]);
  for membership in [Membership::Favorites, Membership::Purchases] {
    assert!(index
      .query(
        &Query {
          kinds: vec!["artist".into()],
          explicit_only: true,
          membership_groups: vec![vec![membership]],
          ..Default::default()
        },
        None,
        100
      )
      .unwrap()
      .items
      .is_empty());
  }
  assert_eq!(
    index
      .query(
        &Query {
          kinds: vec!["artist".into()],
          explicit_only: true,
          exclude_purchases: true,
          ..Default::default()
        },
        None,
        100
      )
      .unwrap()
      .items
      .len(),
    1
  );
  assert_eq!(
    index
      .query(
        &Query {
          kinds: vec!["artist".into()],
          membership_groups: vec![vec![Membership::Followed]],
          ..Default::default()
        },
        None,
        100
      )
      .unwrap()
      .items
      .len(),
    1
  );

  let next = index.begin_generation(&COLLECTIONS, 2).unwrap();
  index.seed_from_current(next, &[(Membership::Followed, "artist".into())]).unwrap();
  publish(&index, next);
  assert!(rows(&index, "artist", true).is_empty());
  let inferred = rows(&index, "artist", false).pop().unwrap();
  assert_eq!(inferred["title"], "Derived purchase");
  assert_eq!(inferred["added_at"], 200);
  assert_eq!(inferred["favorite"], false);
  assert!(index
    .query(
      &Query {
        kinds: vec!["artist".into()],
        membership_groups: vec![vec![Membership::Followed]],
        ..Default::default()
      },
      None,
      100
    )
    .unwrap()
    .items
    .is_empty());
}

#[test]
fn inferred_metadata_is_independent_of_parent_order_and_partial_refresh_history() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let one = parent(1, "Canonical name");
  let mut two = parent(2, "Other name");
  two["artist_image"] = "https://images.invalid/other.jpg".into();
  let first = index.begin_generation(&COLLECTIONS, 1).unwrap();
  add(&index, first, Membership::Favorites, &two, 30);
  add(&index, first, Membership::Favorites, &one, 20);
  let original = publish(&index, first);
  let repeated = index.begin_generation(&COLLECTIONS, 2).unwrap();
  index.seed_from_current(repeated, &[(Membership::Favorites, "album".into())]).unwrap();
  add(&index, repeated, Membership::Favorites, &one, 20);
  add(&index, repeated, Membership::Favorites, &two, 30);
  let equal = publish(&index, repeated);
  assert_eq!((equal.generation, equal.revision), (original.generation, original.revision));
  assert_eq!(rows(&index, "artist", false)[0]["title"], "Canonical name");
  clean(&index);
  let removal = index.begin_generation(&COLLECTIONS, 3).unwrap();
  index.seed_from_current(removal, &[(Membership::Favorites, "album".into())]).unwrap();
  add(&index, removal, Membership::Favorites, &two, 30);
  publish(&index, removal);
  let artist = rows(&index, "artist", false).pop().unwrap();
  assert_eq!(artist["title"], "Other name");
  assert_eq!(artist["image"], two["artist_image"]);
}

#[test]
fn related_artists_keep_their_own_images_and_do_not_turn_into_explicit_favorites() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation = index.begin_generation(&COLLECTIONS, 1).unwrap();
  let mut item = parent(1, "Primary");
  item["library_artists"] = json!([
    {"ref":"artist:1","title":"Primary","image":"https://images.invalid/primary.jpg"},
    {"ref":"artist:2","title":"Interpreter"}
  ]);
  item["label_image"] = "https://images.invalid/label.jpg".into();
  add(&index, generation, Membership::Favorites, &item, 20);
  publish(&index, generation);
  let artists = rows(&index, "artist", false);
  assert_eq!(artists.len(), 2);
  assert!(artists
    .iter()
    .all(|artist| artist["favorite"] == false && artist["library_explicit"] == false));
  assert_eq!(
    artists.iter().find(|artist| artist["ref"] == "artist:1").unwrap()["image"],
    "https://images.invalid/primary.jpg"
  );
  assert!(artists
    .iter()
    .find(|artist| artist["ref"] == "artist:2")
    .unwrap()
    .get("image")
    .is_none());
  assert_eq!(rows(&index, "label", false)[0]["image"], item["label_image"]);
  assert_eq!(
    index
      .query(&Query { explicit_only: true, ..Default::default() }, None, 100)
      .unwrap()
      .items
      .len(),
    1
  );
}

#[test]
fn relationship_expansion_is_bounded_before_a_generation_can_publish() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation = index.begin_generation(&COLLECTIONS, 1).unwrap();
  let mut item = parent(1, "Primary");
  item["library_artists"] = (0..33)
    .map(|number| json!({"ref":format!("artist:{number}"),"title":"Artist"}))
    .collect::<Vec<_>>()
    .into();
  assert!(index
    .ingest(
      generation,
      Membership::Favorites,
      &[IndexedItem { item: &item, added_at: 20, explicit: true }]
    )
    .is_err());
  assert!(index.publish(generation, 10).is_err());
  assert_eq!(index.status().unwrap().generation, 0);
}

#[test]
fn an_old_cache_remains_readable_and_rebuilds_its_projection_only_in_explicit_seed_chunks() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  let generation = index.begin_generation(&COLLECTIONS, 1).unwrap();
  add(&index, generation, Membership::Favorites, &parent(1, "Artist"), 20);
  let before = publish(&index, generation);
  drop(index);
  let path = directory.path().join("library-7.sqlite3");
  let connection = Connection::open(&path).unwrap();
  connection
    .execute_batch(
      "PRAGMA foreign_keys=ON;
    DELETE FROM entities WHERE kind IN ('artist','label');
    UPDATE generations SET item_count=1;
    ALTER TABLE memberships DROP COLUMN origin_kind;
    ALTER TABLE memberships DROP COLUMN origin_reference;
    ALTER TABLE metadata DROP COLUMN projection_version;
    PRAGMA user_version=1;",
    )
    .unwrap();
  drop(connection);
  let reopened = LibraryIndex::open(directory.path(), 7).unwrap();
  assert!(reopened.projection_pending().unwrap());
  assert_eq!(reopened.status().unwrap().generation, before.generation);
  assert_eq!(rows(&reopened, "album", true).len(), 1);
  assert!(
    rows(&reopened, "artist", false).is_empty(),
    "Opening the cache must not scan parent payloads"
  );
  let next = reopened.begin_generation(&COLLECTIONS, 2).unwrap();
  assert!(reopened.seed_from_current_chunk(next, &[], None, 1).unwrap().done);
  publish(&reopened, next);
  assert!(!reopened.projection_pending().unwrap());
  assert_eq!(rows(&reopened, "artist", false).len(), 1);
  drop(reopened);
  let final_index = LibraryIndex::open(directory.path(), 7).unwrap();
  assert!(!final_index.projection_pending().unwrap());
  assert_eq!(rows(&final_index, "label", false).len(), 1);
}

#[test]
fn parent_rebuild_queries_seek_only_the_old_snapshot_candidate_keys() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let connection = index.read();
  for sql in [
    super::derived::PARENTS_SQL,
    super::member::REFRESH_CANDIDATES_SQL,
    super::member::CLEAR_CANDIDATE_GENRES_SQL,
  ] {
    let mut statement = connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let plans: Vec<String> = statement
      .query_map(params![2, 1, "album", "album:008000", "album", "album:008500"], |row| row.get(3))
      .unwrap()
      .collect::<Result<_, _>>()
      .unwrap();
    assert!(
      plans.iter().any(|line| line.contains("SEARCH c USING PRIMARY KEY")
        && line.contains("(kind,reference)>(?,?)")
        && line.contains("(kind,reference)<(?,?)")),
      "{plans:?}"
    );
    assert!(
      !plans.iter().any(|line| line.starts_with("SCAN p") || line.starts_with("SCAN e")),
      "{plans:?}"
    );
  }
}

#[test]
#[ignore = "Run with LIBRARY_INDEX_BENCH_ROWS=100000 or 1000000 and measure process resources externally"]
fn benchmark_inferred_library_index() {
  use std::time::Instant;
  let count: u64 = std::env::var("LIBRARY_INDEX_BENCH_ROWS")
    .ok()
    .and_then(|value| value.parse().ok())
    .unwrap_or(100000);
  assert!(matches!(count, 100000 | 1000000));
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 999).unwrap();
  let fixture = |id: u64| {
    let mut item = parent(id, &format!("Artist {}", id % 1000 + 1));
    item["artist_ref"] = format!("artist:{}", id % 1000 + 1).into();
    item["label_ref"] = format!("label:{}", id % 100 + 1).into();
    item["library_artists"] = json!([
      {"ref":item["artist_ref"],"title":item["artist"]},
      {"ref":format!("artist:{}", (id + 1) % 1000 + 1),"title":format!("Artist {}", (id + 1) % 1000 + 1)}
    ]);
    item
  };
  let ingest_parents = |generation: Generation, include_purchases: bool| {
    for begin in (0..count).step_by(MAX_CHUNK) {
      let items: Vec<_> = (begin..(begin + MAX_CHUNK as u64).min(count)).map(fixture).collect();
      let chunk: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(offset, item)| IndexedItem {
          item,
          added_at: (begin + offset as u64 + 100) as i64,
          explicit: true,
        })
        .collect();
      index.ingest(generation, Membership::Favorites, &chunk).unwrap();
      if include_purchases {
        let purchases: Vec<_> = chunk
          .into_iter()
          .enumerate()
          .filter_map(|(offset, item)| ((begin + offset as u64) % 4 == 0).then_some(item))
          .collect();
        index.ingest(generation, Membership::Purchases, &purchases).unwrap();
      }
    }
  };
  let started = Instant::now();
  let first = index.begin_generation(&COLLECTIONS, 1).unwrap();
  ingest_parents(first, true);
  for number in 1..=50 {
    add(
      &index,
      first,
      Membership::Followed,
      &json!({"kind":"artist","ref":format!("artist:{number}"),"title":format!("Artist {number}")}),
      10,
    );
  }
  let before = publish(&index, first);
  let sync_ms = started.elapsed().as_secs_f64() * 1000.0;
  assert_eq!(before.items, count + 1100);
  let query_started = Instant::now();
  assert_eq!(
    index
      .query(&Query { explicit_only: true, ..Default::default() }, None, 100)
      .unwrap()
      .items
      .len(),
    100
  );
  let all_ms = query_started.elapsed().as_secs_f64() * 1000.0;
  let query_started = Instant::now();
  assert_eq!(rows(&index, "artist", false).len(), 100);
  let artists_ms = query_started.elapsed().as_secs_f64() * 1000.0;
  let following = index
    .query(
      &Query {
        kinds: vec!["artist".into()],
        membership_groups: vec![vec![Membership::Followed]],
        ..Default::default()
      },
      None,
      100,
    )
    .unwrap();
  assert_eq!(following.items.len(), 50);
  let next = index.begin_generation(&COLLECTIONS, 2).unwrap();
  let seed_started = Instant::now();
  let mut cursor = None;
  let mut max_chunk_ms = 0.0_f64;
  loop {
    let chunk_started = Instant::now();
    let progress = index
      .seed_from_current_chunk(
        next,
        &[(Membership::Favorites, "album".into())],
        cursor.as_deref(),
        500,
      )
      .unwrap();
    max_chunk_ms = max_chunk_ms.max(chunk_started.elapsed().as_secs_f64() * 1000.0);
    if progress.done {
      break;
    }
    cursor = progress.next;
  }
  let seed_ms = seed_started.elapsed().as_secs_f64() * 1000.0;
  ingest_parents(next, false);
  let publish_started = Instant::now();
  let equal = publish(&index, next);
  let compare_ms = publish_started.elapsed().as_secs_f64() * 1000.0;
  assert_eq!((equal.generation, equal.revision), (before.generation, before.revision));
  println!(
    "{}",
    json!({"rows":count,"sync_ms":sync_ms,"all_page_ms":all_ms,"artists_page_ms":artists_ms,
    "seed_ms":seed_ms,"seed_max_chunk_ms":max_chunk_ms,"equal_compare_ms":compare_ms,
    "database_bytes":std::fs::metadata(directory.path().join("library-999.sqlite3")).unwrap().len()})
  );
}
