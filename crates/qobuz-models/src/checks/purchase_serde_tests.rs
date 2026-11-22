use crate::types::{PurchaseIdsResponse, PurchaseResponse, PurchaseTrack};

// No captured `/purchase/*` sample exists under `qbz-nix-docs/qobuz-api/`
// (the user cannot exercise a populated-purchase path), so this fixture is
// hand-built to exercise every quirk the source-of-truth §4 calls out:
//   - album `id` arriving as a NUMBER (deserialize_string_id → "12345")
//   - track `id` arriving as a numeric STRING (deserialize_u64_id → 777)
//   - `downloadable` / `streamable` ABSENT → must default TRUE (serde_true)
//   - a malformed optional field (`genre` as a string) → dropped, not fatal
//   - nested album `tracks` page round-trips
//   - `tracks` top-level page MISSING entirely → empty page (lenient_page)
const SAMPLE: &str = r#"{
        "albums": {
            "limit": 50,
            "offset": 0,
            "total": 1,
            "items": [
                {
                    "id": 12345,
                    "title": "Test Album",
                    "artist": { "id": 99, "name": "Test Artist" },
                    "image": { "large": "https://example/large.jpg" },
                    "genre": "not-an-object",
                    "hires": true,
                    "maximum_sampling_rate": 192.0,
                    "maximum_bit_depth": 24,
                    "purchased_at": 1700000000,
                    "tracks": {
                        "limit": 2,
                        "offset": 0,
                        "total": 2,
                        "items": [
                            {
                                "id": "777",
                                "title": "Track One",
                                "track_number": 1,
                                "media_number": 1,
                                "duration": 200
                            },
                            {
                                "id": 778,
                                "title": "Track Two",
                                "track_number": 2,
                                "duration": 210,
                                "streamable": false,
                                "downloaded": true,
                                "downloaded_format_ids": [27, 6]
                            }
                        ]
                    }
                }
            ]
        }
    }"#;

#[test]
fn test_round_trip_real_shaped_sample() {
  let resp: PurchaseResponse =
    serde_json::from_str(SAMPLE).expect("purchase response must deserialize");

  // Missing `tracks` top-level page → empty page, not an error.
  assert_eq!(resp.tracks.items.len(), 0);
  assert_eq!(resp.tracks.total, 0);

  assert_eq!(resp.albums.total, 1);
  assert_eq!(resp.albums.items.len(), 1);
  let album = &resp.albums.items[0];

  // Numeric album id coerced to String.
  assert_eq!(album.id, "12345");
  assert_eq!(album.title, "Test Album");
  assert_eq!(album.artist.id, 99);
  assert_eq!(album.artist.name, "Test Artist");

  // `downloadable` absent → defaults TRUE; `downloaded` absent → false.
  assert!(album.downloadable);
  assert!(!album.downloaded);

  // Malformed `genre` (string instead of object) → dropped, not fatal.
  assert!(album.genre.is_none());

  assert!(album.hires);
  assert_eq!(album.maximum_sampling_rate, Some(192.0));
  assert_eq!(album.maximum_bit_depth, Some(24));
  assert_eq!(album.purchased_at, Some(1_700_000_000));

  // Nested tracks page round-trips.
  let nested = album.tracks.as_ref().expect("nested tracks present");
  assert_eq!(nested.total, 2);
  assert_eq!(nested.items.len(), 2);

  let t1 = &nested.items[0];
  // numeric-string id → u64.
  assert_eq!(t1.id, 777);
  assert_eq!(t1.media_number, Some(1));
  // `streamable` absent → defaults TRUE.
  assert!(t1.streamable);
  assert!(!t1.downloaded);
  assert!(t1.downloaded_format_ids.is_empty());

  let t2 = &nested.items[1];
  assert_eq!(t2.id, 778);
  // `streamable` explicitly false is honored.
  assert!(!t2.streamable);
  assert!(t2.downloaded);
  assert_eq!(t2.downloaded_format_ids, vec![27, 6]);
}

#[test]
fn test_missing_pages_default_empty() {
  // Entirely empty object → both pages empty, no error.
  let resp: PurchaseResponse = serde_json::from_str("{}").expect("empty object must deserialize");
  assert_eq!(resp.albums.items.len(), 0);
  assert_eq!(resp.albums.total, 0);
  assert_eq!(resp.tracks.items.len(), 0);
  assert_eq!(resp.tracks.total, 0);
}

#[test]
fn test_ids_response_reads_only_total() {
  // PurchaseIdsResponse items are opaque; the UI reads only `.total`.
  const IDS: &str = r#"{
            "albums": { "limit": 1, "offset": 0, "total": 42, "items": [ {"foo": 1} ] },
            "tracks": { "limit": 1, "offset": 0, "total": 7,  "items": [ 12345 ] }
        }"#;
  let resp: PurchaseIdsResponse = serde_json::from_str(IDS).expect("ids response must deserialize");
  assert_eq!(resp.albums.total, 42);
  assert_eq!(resp.tracks.total, 7);
}

#[test]
fn test_purchase_track_captures_version_when_present() {
  // INVERTED 2026-08-16 (contract §10-C). This test used to be named
  // `purchase_track_has_no_version_field` and asserted that an incoming
  // `version` key was silently ignored — except it never asserted anything
  // about `version` at all, so it passed either way while documenting the
  // opposite of the behaviour we now want. A guard that guards nothing and
  // misleads the next reader is worse than no guard.
  //
  // `version` is now captured. The purchases ENDPOINTS do not send it, but
  // the album-detail screen builds its tracks from the catalog album, which
  // does, and the reference renders `formatTrackTitle(title, version)`.
  const TRACK: &str = r#"{
            "id": 1,
            "title": "Song",
            "version": "Remastered 2024"
        }"#;
  let track: PurchaseTrack =
    serde_json::from_str(TRACK).expect("track with a version must deserialize");
  assert_eq!(track.id, 1);
  assert_eq!(track.title, "Song");
  assert_eq!(track.version.as_deref(), Some("Remastered 2024"));
  // `streamable` defaults TRUE even on the bare track shape.
  assert!(track.streamable);
}

#[test]
fn test_purchase_track_without_a_version_is_none() {
  // The purchases list shape, which genuinely carries no `version`.
  let track: PurchaseTrack =
    serde_json::from_str(r#"{"id": 2, "title": "Plain"}"#).expect("bare track");
  assert_eq!(track.version, None);
}

#[test]
fn test_radio_response_tolerates_missing_total() {
  use crate::types::RadioResponse;
  // Qobuz `/radio/album` + `/radio/track` return a `tracks` page that
  // OMITS `total` (and sometimes offset/limit). The strict derive used to
  // abort with "missing field `total`"; lenient_page_flexible must default
  // it to the item count and PRESERVE the tracks (not drop them).
  const RADIO: &str = r#"{
            "type": "album",
            "title": "Album Radio",
            "tracks": {
                "items": [
                    { "id": 111, "title": "One" },
                    { "id": 222, "title": "Two" }
                ]
            }
        }"#;
  let resp: RadioResponse =
    serde_json::from_str(RADIO).expect("radio response with missing total must deserialize");
  assert_eq!(resp.tracks.items.len(), 2);
  assert_eq!(resp.tracks.items[0].id, 111);
  // `total` defaulted to the item count.
  assert_eq!(resp.tracks.total, 2);
}

#[test]
fn test_radio_response_absent_tracks_is_empty() {
  use crate::types::RadioResponse;
  // `tracks` key entirely absent → field-level #[serde(default)] → empty
  // page (requires `Track: Default`, added alongside this fix).
  let resp: RadioResponse =
    serde_json::from_str(r#"{ "type": "album" }"#).expect("radio without tracks deserializes");
  assert_eq!(resp.tracks.items.len(), 0);
  assert_eq!(resp.tracks.total, 0);
}
