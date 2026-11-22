use super::*;

#[test]
fn test_user_session_deserializes_pre_v10_json() {
  // Sessions persisted before the country/language capture must still
  // load: both new fields default to None (feature stays Auto-off).
  let json = r#"{
            "user_auth_token": "token",
            "user_id": 1705826,
            "email": "a@b.c",
            "display_name": "Tester",
            "subscription_label": "Studio",
            "subscription_valid_until": null
        }"#;
  let session: UserSession = serde_json::from_str(json).expect("old session json loads");
  assert_eq!(session.country_code, None);
  assert_eq!(session.language_code, None);
  assert_eq!(session.user_id, 1705826);
}

#[test]
fn test_user_session_round_trips_country_and_language() {
  let json = r#"{
            "user_auth_token": "token",
            "user_id": 1705826,
            "email": "a@b.c",
            "display_name": "Tester",
            "subscription_label": "Studio",
            "subscription_valid_until": null,
            "country_code": "FR",
            "language_code": "fr"
        }"#;
  let session: UserSession = serde_json::from_str(json).expect("v10 session json loads");
  assert_eq!(session.country_code.as_deref(), Some("FR"));
  assert_eq!(session.language_code.as_deref(), Some("fr"));
  let back: UserSession = serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
  assert_eq!(back.language_code.as_deref(), Some("fr"));
}

fn image_set() -> ImageSet {
  ImageSet {
    small: Some("s50".into()),
    thumbnail: Some("t150".into()),
    large: Some("l300".into()),
    extralarge: Some("x600".into()),
    mega: Some("m3000".into()),
    back: None,
    medium: None,
  }
}

#[test]
fn test_for_px_picks_the_smallest_covering_variant() {
  let set = image_set();
  assert_eq!(set.for_px(72).map(String::as_str), Some("t150"));
  assert_eq!(set.for_px(150).map(String::as_str), Some("t150"));
  assert_eq!(set.for_px(151).map(String::as_str), Some("l300"));
  assert_eq!(set.for_px(300).map(String::as_str), Some("l300"));
  assert_eq!(set.for_px(660).map(String::as_str), Some("m3000"));
  // Past mega there is nothing bigger: best() is the ceiling.
  assert_eq!(set.for_px(4000).map(String::as_str), Some("m3000"));
}

#[test]
fn test_for_px_skips_missing_variants_upward_on_partial_sets() {
  // Discover endpoints return only small/thumbnail/large.
  let discover = ImageSet {
    small: Some("s50".into()),
    thumbnail: Some("t150".into()),
    large: Some("l300".into()),
    ..ImageSet::default()
  };
  assert_eq!(discover.for_px(72).map(String::as_str), Some("t150"));
  // Nothing >= 600: best-available (large), never an upscale promise.
  assert_eq!(discover.for_px(600).map(String::as_str), Some("l300"));
  // A gap below the request keeps scanning UP: {small, mega} at 200px
  // is served by mega — the smallest variant that COVERS the slot (the
  // "never upscale into a slot" rule wins over download size; full
  // album/track sets populate all five variants, so this only bites on
  // genuinely sparse sets).
  let gapped =
    ImageSet { small: Some("s50".into()), mega: Some("m3000".into()), ..ImageSet::default() };
  assert_eq!(gapped.for_px(200).map(String::as_str), Some("m3000"));
  assert_eq!(gapped.for_px(72).map(String::as_str), Some("m3000"));
}

#[test]
fn test_for_px_never_serves_worse_than_best() {
  let tiny = ImageSet { small: Some("s50".into()), ..ImageSet::default() };
  assert_eq!(tiny.for_px(1600).map(String::as_str), Some("s50"));
  assert_eq!(ImageSet::default().for_px(300), None);
}

#[test]
fn test_bucket_for_px_maps_slots_onto_the_variant_table() {
  assert_eq!(ImageSet::bucket_for_px(0), IMAGE_VARIANT_PX[0]);
  assert_eq!(ImageSet::bucket_for_px(72), 150);
  assert_eq!(ImageSet::bucket_for_px(150), 150);
  assert_eq!(ImageSet::bucket_for_px(151), 300);
  assert_eq!(ImageSet::bucket_for_px(660), 3000);
  // Beyond the table the bucket saturates at mega, so a 4K window does
  // not invent a size no variant serves.
  assert_eq!(ImageSet::bucket_for_px(5000), 3000);
}

#[test]
fn test_qobuz_cover_at_px_rewrites_the_size_suffix() {
  let small = "https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_50.jpg";
  assert_eq!(
    qobuz_cover_at_px(small, 600).as_deref(),
    Some("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_600.jpg")
  );
  // Slots bucket UP to the smallest covering variant.
  assert_eq!(
    qobuz_cover_at_px(small, 250).as_deref(),
    Some("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_300.jpg")
  );
  assert_eq!(
    qobuz_cover_at_px(small, 5000).as_deref(),
    Some("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_max.jpg")
  );
  // The mega bucket emits the literal `_max` suffix, never `_3000`
  // (a 404 on the live CDN, measured 2026-08-15).
  assert_eq!(
    qobuz_cover_at_px(small, 700).as_deref(),
    Some("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_max.jpg")
  );
  // Down-tiering works too (a mega `_max` carried into a 150px slot).
  let mega = "https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_max.jpg";
  assert_eq!(
    qobuz_cover_at_px(mega, 100).as_deref(),
    Some("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_150.jpg")
  );
}

#[test]
fn test_qobuz_cover_at_px_leaves_unrecognized_urls_alone() {
  // Not a cover url, no size suffix, an unknown size, a query string —
  // all None, so callers keep the original.
  assert_eq!(qobuz_cover_at_px("https://example.com/a_50.jpg", 600), None);
  assert_eq!(
    qobuz_cover_at_px("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb.jpg", 600),
    None
  );
  assert_eq!(
    qobuz_cover_at_px("https://static.qobuz.com/images/covers/pb/ap/gxy13gb56appb_123.jpg", 600),
    None
  );
  assert_eq!(
    qobuz_cover_at_px("https://static.qobuz.com/images/covers/pb/ap/x_50.jpg?token=1", 600),
    None
  );
  assert_eq!(qobuz_cover_at_px("/music/cover.jpg", 600), None);
}
