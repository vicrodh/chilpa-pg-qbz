use super::*;
use serde_json::json;

fn add(
  index: &LibraryIndex,
  generation: Generation,
  collection: Membership,
  item: &Value,
  added_at: i64,
  explicit: bool,
) {
  index.ingest(generation, collection, &[IndexedItem { item, added_at, explicit }]).unwrap();
}

#[test]
fn changed_source_seeding_retains_other_memberships_and_recomputes_all_ordering() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let collections = [Membership::Favorites, Membership::Purchases, Membership::Followed];
  let first = index.begin_generation(&collections, 1).unwrap();
  let shared = json!({"kind":"album","ref":"album:1","title":"Shared"});
  let removed = json!({"kind":"album","ref":"album:2","title":"Removed"});
  let purchase = json!({"kind":"track","ref":"track:3","title":"Purchased"});
  let inferred = json!({"kind":"artist","ref":"artist:4","title":"Inferred"});
  add(&index, first, Membership::Favorites, &shared, 30, true);
  add(&index, first, Membership::Purchases, &shared, 10, true);
  add(&index, first, Membership::Favorites, &removed, 20, true);
  add(&index, first, Membership::Purchases, &purchase, 15, true);
  add(&index, first, Membership::Followed, &inferred, 50, false);
  for collection in collections {
    index.finish_collection(first, collection).unwrap();
  }
  index.publish(first, 2).unwrap();
  let next = index.begin_generation(&collections, 3).unwrap();
  let copied = index.seed_from_current(next, &[(Membership::Favorites, "album".into())]).unwrap();
  assert_eq!(copied, IngestStats { new_entities: 3, new_memberships: 3 });
  assert_eq!(
    index
      .query(&Query { explicit_only: true, ..Default::default() }, None, 100)
      .unwrap()
      .items
      .len(),
    3,
    "Staging must not replace the published page"
  );
  let added = json!({"kind":"album","ref":"album:5","title":"Added"});
  add(&index, next, Membership::Favorites, &added, 40, true);
  for collection in collections {
    index.finish_collection(next, collection).unwrap();
  }
  index.publish(next, 4).unwrap();
  let page = index.query(&Query { explicit_only: true, ..Default::default() }, None, 100).unwrap();
  assert_eq!(
    page.items.iter().map(|item| item["ref"].as_str().unwrap()).collect::<Vec<_>>(),
    vec!["album:5", "track:3", "album:1"]
  );
  assert_eq!(page.items[2]["added_at"], 10);
  assert_eq!(page.items[2]["favorite"], false);
  assert_eq!(page.items[2]["purchased"], true);
  assert_eq!(index.query(&Query::default(), None, 100).unwrap().items.len(), 4);
}

#[test]
fn revision_only_refresh_updates_stamps_and_freshness_without_changing_the_data_revision() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let first = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  let item = json!({"kind":"album","ref":"album:1","title":"Record"});
  add(&index, first, Membership::Favorites, &item, 10, true);
  let stamps = Revisions::from([("albums".into(), Some(20)), ("playlists".into(), None)]);
  index.stage_revisions(first, &stamps).unwrap();
  index.finish_collection(first, Membership::Favorites).unwrap();
  let original = index.publish(first, 30).unwrap();
  assert_eq!(index.revisions().unwrap(), stamps);
  let second = index.begin_generation(&[Membership::Favorites], 31).unwrap();
  index.seed_from_current(second, &[]).unwrap();
  let updated = Revisions::from([("albums".into(), Some(21)), ("playlists".into(), None)]);
  index.stage_revisions(second, &updated).unwrap();
  index.finish_collection(second, Membership::Favorites).unwrap();
  let status = index.publish(second, 40).unwrap();
  assert_eq!(status.generation, original.generation);
  assert_eq!(status.revision, original.revision);
  assert_eq!(index.revisions().unwrap(), updated);
  index.mark_checked(50).unwrap();
  let checked = index.status().unwrap();
  assert_eq!(checked.checked_at, 50);
  assert_eq!((checked.generation, checked.revision), (original.generation, original.revision));
  let reader = index.read();
  assert_eq!(reader.pragma_query_value(None, "query_only", |row| row.get::<_, i64>(0)).unwrap(), 1);
  assert_eq!(
    reader.pragma_query_value(None, "cache_size", |row| row.get::<_, i64>(0)).unwrap(),
    -4096
  );
}

#[test]
fn partial_and_full_refresh_choose_the_same_payload_and_removing_a_source_recovers_its_peer() {
  let directory = tempfile::tempdir().unwrap();
  let full_directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let full = LibraryIndex::open(full_directory.path(), 1).unwrap();
  let collections = [Membership::Favorites, Membership::Purchases];
  let favorite = json!({"kind":"album","ref":"album:1","title":"Favorite one",
    "image":"favorite.jpg","genre":"Old favorite","hires":true});
  let revised = json!({"kind":"album","ref":"album:1","title":"Álbum Favorito Dos",
    "genre":"Jazz","genre_ids":[123],"hires":false,"owner_id":"11"});
  let purchase = json!({"kind":"album","ref":"album:1","title":"Purchased edition",
    "image":"purchase.jpg","genre":"Purchase genre","genre_ids":[456],"owner_id":"22",
    "quality":{"bits":24,"rate":96000}});
  let finish = |index: &LibraryIndex, generation| {
    for collection in collections {
      index.finish_collection(generation, collection).unwrap();
    }
    index.publish(generation, 10).unwrap()
  };
  let first = index.begin_generation(&collections, 1).unwrap();
  add(&index, first, Membership::Favorites, &favorite, 30, true);
  add(&index, first, Membership::Purchases, &purchase, 20, true);
  finish(&index, first);

  let partial = index.begin_generation(&collections, 2).unwrap();
  index.seed_from_current(partial, &[(Membership::Favorites, "album".into())]).unwrap();
  add(&index, partial, Membership::Favorites, &revised, 30, true);
  finish(&index, partial);
  let complete = full.begin_generation(&collections, 2).unwrap();
  add(&full, complete, Membership::Favorites, &revised, 30, true);
  add(&full, complete, Membership::Purchases, &purchase, 20, true);
  let complete_status = finish(&full, complete);
  let expected = full.query(&Query::default(), None, 100).unwrap().items;
  assert_eq!(index.query(&Query::default(), None, 100).unwrap().items, expected);
  assert_eq!(expected[0]["title"], "Purchased edition");

  let reversed = full.begin_generation(&collections, 3).unwrap();
  add(&full, reversed, Membership::Purchases, &purchase, 20, true);
  add(&full, reversed, Membership::Favorites, &revised, 30, true);
  let reversed_status = finish(&full, reversed);
  assert_eq!(reversed_status.generation, complete_status.generation);
  assert_eq!(reversed_status.revision, complete_status.revision);
  assert!(!index.prune_chunk(500).unwrap().pending);
  assert!(!full.prune_chunk(500).unwrap().pending);

  let removal = index.begin_generation(&collections, 4).unwrap();
  index.seed_from_current(removal, &[(Membership::Purchases, "album".into())]).unwrap();
  finish(&index, removal);
  let favorite_only = full.begin_generation(&collections, 4).unwrap();
  add(&full, favorite_only, Membership::Favorites, &revised, 30, true);
  finish(&full, favorite_only);
  let recovered = index.query(&Query::default(), None, 100).unwrap().items;
  assert_eq!(recovered, full.query(&Query::default(), None, 100).unwrap().items);
  assert_eq!(recovered[0]["title"], "Álbum Favorito Dos");
  assert!(recovered[0].get("image").is_none());
  assert!(recovered[0].get("quality").is_none());
  assert_eq!(recovered[0]["hires"], false);
  for query in [
    Query { genres: vec!["456".into()], ..Default::default() },
    Query { owners: vec!["22".into()], ..Default::default() },
    Query { hires: Some(true), ..Default::default() },
    Query { text: "Purchased edition".into(), ..Default::default() },
  ] {
    assert!(index.query(&query, None, 100).unwrap().items.is_empty());
  }
  assert_eq!(
    index
      .query(&Query { genres: vec!["123".into()], ..Default::default() }, None, 100)
      .unwrap()
      .items
      .len(),
    1
  );
}

#[test]
fn seed_chunks_advance_over_removed_candidates_and_never_publish_an_unfinished_seed() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let collections = [Membership::Favorites, Membership::Purchases];
  let first = index.begin_generation(&collections, 1).unwrap();
  for number in 0..501 {
    let item =
      json!({"kind":"album","ref":format!("album:{number:04}"),"title":format!("Record {number}")});
    add(&index, first, Membership::Favorites, &item, number, true);
    if number == 500 {
      add(&index, first, Membership::Purchases, &item, 1, true);
    }
  }
  for collection in collections {
    index.finish_collection(first, collection).unwrap();
  }
  let initial = index.publish(first, 2).unwrap();
  let next = index.begin_generation(&collections, 3).unwrap();
  let replaced = [(Membership::Favorites, "album".into())];
  let chunk = index.seed_from_current_chunk(next, &replaced, None, usize::MAX).unwrap();
  assert!(!chunk.done && chunk.next.is_some());
  assert_eq!(chunk.stats, IngestStats::default(), "Removed candidates still advance the keyset");
  assert_eq!(index.query(&Query::default(), None, 100).unwrap().status, initial);
  for collection in collections {
    index.finish_collection(next, collection).unwrap();
  }
  assert!(index.publish(next, 4).is_err());
  let final_chunk =
    index.seed_from_current_chunk(next, &replaced, chunk.next.as_deref(), 500).unwrap();
  assert!(final_chunk.done && final_chunk.next.is_none());
  assert_eq!(final_chunk.stats, IngestStats { new_entities: 1, new_memberships: 1 });
  index.publish(next, 4).unwrap();
  let page = index.query(&Query::default(), None, 100).unwrap();
  assert_eq!(page.items.len(), 1);
  assert_eq!(page.items[0]["ref"], "album:0500");
  assert_eq!(page.items[0]["favorite"], false);
  assert_eq!(page.items[0]["purchased"], true);
}

#[test]
fn seed_cursor_cannot_restart_a_partially_copied_generation() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let first = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  for number in 0..3 {
    let item = json!({"kind":"album","ref":format!("album:{number}"),"title":"Record"});
    add(&index, first, Membership::Favorites, &item, number, true);
  }
  index.finish_collection(first, Membership::Favorites).unwrap();
  let initial = index.publish(first, 2).unwrap();
  let next = index.begin_generation(&[Membership::Favorites], 3).unwrap();
  let copied = index.seed_from_current_chunk(next, &[], None, 1).unwrap();
  assert_eq!(copied.stats.new_entities, 1);
  assert!(index.seed_from_current_chunk(next, &[], None, 1).is_err());
  assert!(index.publish(next, 4).is_err());
  assert_eq!(index.status().unwrap(), initial);
}

#[test]
fn seed_insert_plans_seek_the_complete_candidate_key_before_joining_memberships() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 1).unwrap();
  let generation = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  for replaced in [
    vec![],
    vec![(Membership::Favorites, "album".into())],
    vec![
      (Membership::Favorites, "album".into()),
      (Membership::Favorites, "track".into()),
      (Membership::Favorites, "playlist".into()),
      (Membership::Followed, "artist".into()),
      (Membership::Followed, "label".into()),
      (Membership::Followed, "award".into()),
      (Membership::Purchases, "album".into()),
      (Membership::Purchases, "track".into()),
    ],
  ] {
    let plans = index
      .explain_seed_chunk(
        generation,
        &replaced,
        ("album", "album:000000008000"),
        ("album", "album:000000008500"),
      )
      .unwrap();
    for (position, plan) in plans.iter().enumerate() {
      let driver = match position {
        1 => "m",
        2 => "c",
        _ => "e",
      };
      assert!(
        plan.iter().any(|line| line.contains(&format!("SEARCH {driver} USING PRIMARY KEY"))
          && line.contains("(kind,reference)>(?,?)")
          && line.contains("(kind,reference)<(?,?)")),
        "The chunk must seek both ends of its full candidate key: {plan:?}"
      );
      if position != 1 {
        assert!(
          plan
            .iter()
            .filter(|line| line.contains("SEARCH m "))
            .all(|line| line.contains("kind=? AND reference=?")),
          "A membership join must look up one candidate, not scan its entire kind: {plan:?}"
        );
        assert!(
          plan.iter().any(|line| line.contains("SEARCH m USING PRIMARY KEY")
            && line.contains("kind=? AND reference=? AND collection=?")),
          "{plan:?}"
        );
      }
    }
  }
}
