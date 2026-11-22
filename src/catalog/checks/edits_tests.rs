use super::*;
#[test]
fn test_member_selection() {
  let playlist: qobuz_models::Playlist = serde_json::from_value(json!({"tracks":{"total":2,"items":[{"id":7,"playlist_track_id":81},{"id":7,"playlist_track_id":82}]}})).unwrap();
  assert_eq!(
    members(&json!({"members":["81","82","81"]}), &playlist).unwrap_or_default(),
    vec![81, 82]
  );
  assert!(members(&json!({"members":["7"]}), &playlist).is_err());
  assert!(members(&json!({"members":[81]}), &playlist).is_err());
  assert!(members(&json!({"members":[]}), &playlist).is_err());
}
#[test]
fn test_duplicate_rows() {
  let a = entry(&json!({"id":7,"playlist_track_id":81}), 2, 0);
  let b = entry(&json!({"id":7,"playlist_track_id":82}), 2, 1);
  assert_ne!(a["ref"], b["ref"]);
  assert_eq!(numeric_ref(&a, "track").unwrap_or_default(), 7);
  assert_eq!(a["member"], "81");
  assert_eq!(a["ref"].as_str().unwrap().split('@').nth(1), Some("playlist:2"));
  assert_eq!(entry(&json!({"id":7}), 2, 0)["member"], Value::Null);
}
#[test]
fn test_owner_flags() {
  let value = json!({"id":1,"owner":{"id":8},"is_public":true});
  assert_eq!(card(&value, 8)["editable"], true);
  assert_eq!(card(&value, 9)["editable"], false);
  assert_eq!(card(&json!({"owner":{"id":0}}), 0)["editable"], false);
}
