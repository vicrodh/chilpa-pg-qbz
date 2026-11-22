use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct Item {
  id: u64,
  name: String,
}

#[test]
fn test_lenient_keeps_good_items_and_drops_bad_ones() {
  let items = vec![
    serde_json::json!({ "id": 1, "name": "a" }),
    serde_json::json!({ "id": "not-a-number", "name": "poisoned" }),
    serde_json::json!({ "id": 2, "name": "b" }),
  ];
  let out: Vec<Item> = parse_items_lenient(items, "test");
  assert_eq!(out.len(), 2);
  assert_eq!(out[0].id, 1);
  assert_eq!(out[1].id, 2);
}

#[test]
fn test_array_helper_walks_the_envelope() {
  let value = serde_json::json!({
      "tracks": {
          "items": [
              { "id": 7, "name": "keep" },
              { "id": null, "name": "drop" }
          ],
          "total": 2
      }
  });
  let out: Vec<Item> = parse_items_array(&value, "tracks", "test");
  assert_eq!(out, vec![Item { id: 7, name: "keep".into() }]);
}

#[test]
fn test_array_helper_handles_missing_envelope() {
  let out: Vec<Item> = parse_items_array(&serde_json::json!({}), "tracks", "test");
  assert!(out.is_empty());
  let out: Vec<Item> = parse_items_array(&serde_json::json!({ "tracks": null }), "tracks", "test");
  assert!(out.is_empty());
}
