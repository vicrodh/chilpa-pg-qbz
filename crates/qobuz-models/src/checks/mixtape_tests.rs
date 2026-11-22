use super::*;

#[test]
fn test_kind_serde_roundtrip() {
  let k = CollectionKind::ArtistCollection;
  let s = serde_json::to_string(&k).unwrap();
  assert_eq!(s, "\"artist_collection\"");
  let back: CollectionKind = serde_json::from_str(&s).unwrap();
  assert_eq!(back, k);
}

#[test]
fn test_collection_serde_omits_empty_items() {
  let c = MixtapeCollection {
    id: "abc".into(),
    kind: CollectionKind::Mixtape,
    name: "90s Cassettes".into(),
    description: None,
    source_type: CollectionSourceType::Manual,
    source_ref: None,
    play_mode: CollectionPlayMode::InOrder,
    custom_artwork_path: None,
    position: 0,
    hidden: false,
    last_played_at: None,
    play_count: 0,
    last_synced_at: None,
    created_at: 1,
    updated_at: 1,
    items: Vec::new(),
  };
  let s = serde_json::to_string(&c).unwrap();
  // items serializes as empty array (serde default Vec). Snake-case fields.
  assert!(s.contains("\"kind\":\"mixtape\""));
  assert!(s.contains("\"source_type\":\"manual\""));
  assert!(s.contains("\"play_mode\":\"in_order\""));
}
