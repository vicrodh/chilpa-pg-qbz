use super::*;

#[test]
fn test_link_routes() {
  assert_eq!(
    classify_link("https://play.qobuz.com/album/0060254773574"),
    json!({"kind": "album", "ref": "album:0060254773574"})
  );
  assert_eq!(
    classify_link("https://open.qobuz.com/track/12345"),
    json!({"kind": "track", "ref": "track:12345"})
  );
  assert_eq!(classify_link("qobuzapp://artist/42"), json!({"kind": "artist", "ref": "artist:42"}));
  assert_eq!(
    classify_link("https://play.qobuz.com/playlist/77"),
    json!({"kind": "playlist", "ref": "playlist:77"})
  );
  assert!(classify_link("https://example.org/album/1").is_null());
  assert!(classify_link("").is_null());
}

#[test]
fn test_reference_validation() {
  assert_eq!(parse_reference(&json!({"ref": "album:x1"}), "album").ok(), Some("x1".to_string()));
  assert!(parse_reference(&json!({"ref": "track:1"}), "album").is_err());
  assert!(parse_reference(&json!({"ref": "album:"}), "album").is_err());
}
