use super::*;
use serde_json::json;
use std::collections::BTreeMap;

fn stage(index: &LibraryIndex, title: &str) -> Generation {
  let generation = index.begin_generation(&[Membership::Favorites], 1).unwrap();
  let item = json!({"kind":"album","ref":"album:1","title":title});
  index
    .ingest(
      generation,
      Membership::Favorites,
      &[IndexedItem { item: &item, added_at: 10, explicit: true }],
    )
    .unwrap();
  index.finish_collection(generation, Membership::Favorites).unwrap();
  generation
}

#[test]
fn durable_local_versions_survive_restart_and_an_old_ack_preserves_a_newer_mutation() {
  let directory = tempfile::tempdir().unwrap();
  assert!(!LibraryIndex::exists(directory.path(), 7).unwrap());
  assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  assert!(LibraryIndex::exists(directory.path(), 7).unwrap());
  let first = stage(&index, "One");
  let initial = index.publish(first, 20).unwrap();
  assert_eq!(index.mark_local_changed("favorite-albums").unwrap(), 1);
  let captured = BTreeMap::from([("favorite-albums".into(), 1)]);
  assert_eq!(index.mark_local_changed("favorite-albums").unwrap(), 2);
  let repeated = stage(&index, "One");
  let equal = index.publish_with_local_versions(repeated, 30, &captured).unwrap();
  assert_eq!((equal.generation, equal.revision), (initial.generation, initial.revision));
  assert_eq!(
    index.local_versions().unwrap()["favorite-albums"],
    LocalVersion { version: 2, applied: 1 }
  );
  drop(index);

  let reopened = LibraryIndex::open(directory.path(), 7).unwrap();
  assert_eq!(
    reopened.local_versions().unwrap()["favorite-albums"],
    LocalVersion { version: 2, applied: 1 }
  );
  assert!(!reopened.prune_chunk(500).unwrap().pending);
  let changed = stage(&reopened, "Two");
  let status = reopened
    .publish_with_local_versions(changed, 40, &BTreeMap::from([("favorite-albums".into(), 2)]))
    .unwrap();
  assert_ne!(status.generation, initial.generation);
  drop(reopened);
  let completed = LibraryIndex::open(directory.path(), 7).unwrap();
  assert_eq!(
    completed.local_versions().unwrap()["favorite-albums"],
    LocalVersion { version: 2, applied: 2 }
  );
  assert!(LibraryIndex::open(directory.path(), 8).unwrap().local_versions().unwrap().is_empty());
}

#[test]
fn publication_and_all_local_acknowledgements_roll_back_together() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  let first = stage(&index, "One");
  let before = index.publish(first, 20).unwrap();
  index.mark_local_changed("a-albums").unwrap();
  index.mark_local_changed("b-tracks").unwrap();
  let changed = stage(&index, "Two");
  let invalid = BTreeMap::from([("a-albums".into(), 1), ("b-tracks".into(), 2)]);
  assert!(index.publish_with_local_versions(changed, 30, &invalid).is_err());
  assert_eq!(index.status().unwrap(), before);
  for version in index.local_versions().unwrap().values() {
    assert_eq!(*version, LocalVersion { version: 1, applied: 0 });
  }
  index
    .publish_with_local_versions(
      changed,
      30,
      &BTreeMap::from([("a-albums".into(), 1), ("b-tracks".into(), 1)]),
    )
    .unwrap();
  for version in index.local_versions().unwrap().values() {
    assert_eq!(*version, LocalVersion { version: 1, applied: 1 });
  }
}

#[test]
fn local_markers_have_bounded_names_and_counts_and_plain_publish_does_not_ack_them() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  for invalid in ["", "with space", "source/name"] {
    assert!(index.mark_local_changed(invalid).is_err());
  }
  assert!(index.mark_local_changed(&"x".repeat(65)).is_err());
  for number in 0..32 {
    index.mark_local_changed(&format!("source-{number}")).unwrap();
  }
  assert!(index.mark_local_changed("one-too-many").is_err());
  assert_eq!(index.mark_local_changed("source-0").unwrap(), 2);
  let generation = stage(&index, "One");
  index.publish(generation, 20).unwrap();
  assert_eq!(index.local_versions().unwrap().len(), 32);
  assert!(index.local_versions().unwrap().values().all(|version| version.applied == 0));
}
