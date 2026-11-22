use super::*;

#[test]
fn test_spotlight_stays_on_the_same_artist_across_refresh_seconds() {
  let artists: Vec<_> =
    (1..=5).map(|id| json!({"kind":"artist","ref":format!("artist:{id}")})).collect();
  let stamp = crate::catalog::now();
  let first = spotlight_artist(&artists);
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
  while crate::catalog::now() == stamp {
    assert!(
      std::time::Instant::now() < deadline,
      "The clock must advance for the refresh comparison"
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
  }
  assert_eq!(
    spotlight_artist(&artists),
    first,
    "A refresh with the same favourites must keep the spotlight artist"
  );
}

#[test]
fn test_spotlight_uses_a_valid_favourite_or_stays_empty() {
  assert_eq!(spotlight_artist(&[]), None);
  let artists =
    vec![json!({"ref":"album:7"}), json!({"ref":"artist:invalid"}), json!({"ref":"artist:42"})];
  assert_eq!(spotlight_artist(&artists), Some(42));
}
