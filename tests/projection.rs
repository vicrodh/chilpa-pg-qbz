use pg_qbz::projection;
use serde_json::Value;

fn corpus() -> Vec<Value> {
  serde_json::from_str(include_str!("fixtures/projection.json")).unwrap()
}

#[test]
fn test_projection_corpus() {
  for case in corpus() {
    let input = &case["input"];
    let parent = (!case["parent"].is_null()).then_some(&case["parent"]);
    let actual = match case["kind"].as_str().unwrap() {
      "album" => projection::album_card(input),
      "track" => projection::track_card(input, parent),
      "artist" => projection::artist_card(input),
      "label" => projection::label_card(input),
      "playlist" => projection::playlist_card(input),
      _ => panic!("Unknown corpus kind"),
    };
    assert_eq!(actual, case["expected"], "{}", case["case"]);
  }
}

#[test]
fn test_wire_roundtrip_keeps_library_metadata() {
  for case in
    corpus().into_iter().filter(|case| matches!(case["case"].as_str(), Some("album-2" | "track-3")))
  {
    let (actual, expected) = if case["kind"] == "album" {
      let record: qobuz_models::Album = serde_json::from_value(case["input"].clone()).unwrap();
      (projection::album_card(&serde_json::to_value(record).unwrap()), &case["expected"])
    } else {
      let record: qobuz_models::Track = serde_json::from_value(case["input"].clone()).unwrap();
      (
        projection::track_card(&serde_json::to_value(record).unwrap(), Some(&case["parent"])),
        &case["expected"],
      )
    };
    for key in [
      "added_at",
      "release_date",
      "genre",
      "genres",
      "label",
      "label_ref",
      "release_type",
      "composers",
      "work",
      "performers",
      "normalization",
    ] {
      assert_eq!(actual[key], expected[key], "{} field {key}", case["case"]);
    }
  }
}

#[test]
fn structured_track_artists_survive_typed_decoding_and_projection() {
  let input = serde_json::json!({"id":42,"title":"Record","track_number":1,
    "performer":{"id":99,"name":"GEORGE HARRISON, Phil McDonald, Big Sky Music"},
    "artists":[{"id":1,"name":"George Harrison","roles":["performer"]},
      {"id":2,"name":"Phil McDonald","roles":["engineer"]}],
    "performers":"George Harrison, MainArtist - Phil McDonald, Engineer"});
  let track: qobuz_models::Track =
    qobuz_models::artist_display::from_catalog_value(input.clone()).unwrap();
  for value in [input, serde_json::to_value(track).unwrap()] {
    let card = projection::track_card(&value, None);
    assert_eq!(card["artist"], "George Harrison");
    assert_eq!(card["artist_ref"], "artist:1");
    assert_eq!(card["performers"], "George Harrison, MainArtist - Phil McDonald, Engineer");
  }
}

#[test]
fn playlist_editorial_artwork_survives_detail_decoding() {
  for (field, address) in [
    ("image_rectangle", "https://example.org/editorial.jpg"),
    ("image_rectangle_mini", "https://example.org/editorial-mini.jpg"),
  ] {
    let mut input = serde_json::json!({"id":9,"name":"Editorial playlist",
      "images300":["https://example.org/album.jpg"],
      "images150":["https://example.org/album-small.jpg"],
      "images":["https://example.org/album.jpg"]});
    input[field] = serde_json::json!([address]);
    let detail: qobuz_models::Playlist = serde_json::from_value(input.clone()).unwrap();
    for value in [input, serde_json::to_value(detail).unwrap()] {
      let card = projection::playlist_card(&value);
      assert_eq!(card["image"], address);
      assert_eq!(card["art_fit"], "contain");
    }
  }
}

#[test]
fn playlist_artwork_prefers_full_editorial_image_and_keeps_album_fallback() {
  let mut input = serde_json::json!({"id":9,"name":"Playlist",
    "image_rectangle":["//example.org/editorial.jpg"],
    "image_rectangle_mini":["https://example.org/editorial-mini.jpg"],
    "images300":["https://example.org/album.jpg"]});
  assert_eq!(projection::playlist_card(&input)["image"], "https://example.org/editorial.jpg");
  input["image_rectangle"] = serde_json::json!([]);
  assert_eq!(projection::playlist_card(&input)["image"], "https://example.org/editorial-mini.jpg");
  input["image_rectangle_mini"] = serde_json::Value::Null;
  assert_eq!(projection::playlist_card(&input)["image"], "https://example.org/album.jpg");
}

#[test]
fn album_artwork_is_consistent_between_discovery_detail_and_tracks() {
  let discovery = serde_json::json!({"id":"record","title":"Record","artists":[],
    "image":{"large":"https://example.org/record_600.jpg","small":"https://example.org/record_300.jpg"}});
  let mut detail = discovery.clone();
  detail["image"]["mega"] = serde_json::json!("https://example.org/record_max.jpg");
  detail["image"]["extralarge"] = serde_json::json!("https://example.org/record_1200.jpg");
  let discovered: qobuz_models::DiscoverAlbum = serde_json::from_value(discovery).unwrap();
  let album: qobuz_models::Album = serde_json::from_value(detail).unwrap();
  let discovered = serde_json::to_value(discovered).unwrap();
  let album = serde_json::to_value(album).unwrap();
  let track = serde_json::json!({"id":7,"title":"Song","album":album});
  let expected = projection::album_card(&discovered)["image"].clone();
  assert_eq!(projection::album_card(&album)["image"], expected);
  assert_eq!(projection::track_card(&track, None)["image"], expected);
}

#[test]
fn album_cover_tiers_share_a_sufficient_catalog_image_without_rewriting_other_hosts() {
  let expected = "https://static.qobuz.com/images/covers/00/11/record_600.jpg";
  for (tier, suffix) in [("large", "300"), ("extralarge", "600"), ("mega", "max")] {
    let mut album = serde_json::json!({"id":"record","title":"Record"});
    album["image"] = serde_json::json!({});
    album["image"][tier] = serde_json::json!(format!(
      "https://static.qobuz.com/images/covers/00/11/record_{suffix}.jpg"
    ));
    assert_eq!(projection::album_card(&album)["image"], expected);
  }
  let image = "https://example.org/images/covers/record_max.jpg";
  assert_eq!(projection::album_card(&serde_json::json!({"image":{"mega":image}}))["image"], image);
  let image = "https://static.qobuz.com/images/covers/record.jpg";
  assert_eq!(projection::album_card(&serde_json::json!({"image":{"large":image}}))["image"], image);
}
