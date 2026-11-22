use super::{upcoming_from, Track};
use chrono::{TimeZone, Utc};

#[test]
fn test_future_streamable_at_wins_over_the_date() {
  let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
  assert!(upcoming_from(Some(now.timestamp() + 60), None, now));
  assert!(upcoming_from(Some(now.timestamp() + 60), Some("2000-01-01"), now));
  assert!(!upcoming_from(Some(now.timestamp() - 60), Some("2099-01-01"), now));
  // The captured dead track: `streamable_at: null`, past release date.
  assert!(!upcoming_from(None, Some("2013-06-21"), now));
}

#[test]
fn test_release_date_is_the_fallback_and_absence_is_not_upcoming() {
  let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
  assert!(upcoming_from(None, Some("2026-10-16"), now));
  assert!(!upcoming_from(None, Some("2026-09-02"), now), "today is not upcoming");
  assert!(!upcoming_from(None, Some("not a date"), now));
  assert!(!upcoming_from(None, None, now));
}

#[test]
fn test_streamable_track_is_never_upcoming() {
  let track = Track {
    streamable: Some(true),
    release_date_stream: Some("2099-01-01".into()),
    ..Default::default()
  };
  assert!(!track.is_upcoming());
  let track = Track {
    streamable: Some(false),
    release_date_stream: Some("2099-01-01".into()),
    ..Default::default()
  };
  assert!(track.is_upcoming());
}
