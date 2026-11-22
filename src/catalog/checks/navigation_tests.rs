use super::*;

fn category(id: &str, record: u64) -> Value {
  json!({"id":id,"data":{"items":[{"id":record.to_string(),"title":format!("Record {record}")}]}})
}

#[test]
fn test_label_modules_keep_their_full_association_when_a_category_is_inserted_or_moved() {
  let albums = category("allReleases", 1);
  let upcoming = category("nextReleases", 2);
  let awards = category("awardedReleases", 3);
  let original = label_release_shelves(9, &json!({"releases":[albums,upcoming]}));
  let refreshed = label_release_shelves(9, &json!({"releases":[awards,upcoming,albums]}));
  for module in original {
    let current = refreshed.iter().find(|row| row["title"] == module["title"]).unwrap();
    assert_eq!(
      current, &module,
      "An existing label module must keep its identity, rows and continuation"
    );
  }
  assert_eq!(refreshed[0]["more"], "label:9:awards:0");
}

#[test]
fn test_unnamed_label_modules_use_content_identity_without_sharing_a_slot() {
  let mut first = category("", 1);
  let mut second = category("", 2);
  first.as_object_mut().unwrap().remove("id");
  second.as_object_mut().unwrap().remove("id");
  let original = label_release_shelves(9, &json!({"releases":[first,second]}));
  let refreshed = label_release_shelves(9, &json!({"releases":[second,first]}));
  assert_ne!(original[0]["id"], original[1]["id"]);
  assert_eq!(original[0], refreshed[1]);
  assert_eq!(original[1], refreshed[0]);
}

#[test]
fn test_named_label_modules_keep_identity_when_contents_change_and_ignore_duplicate_ids() {
  let original = label_release_shelves(9, &json!({"releases":[category("allReleases",1)]}));
  let current = label_release_shelves(
    9,
    &json!({"releases":[category("allReleases",2),category("allReleases",3)]}),
  );
  assert_eq!(current.len(), 1);
  assert_eq!(current[0]["id"], original[0]["id"]);
  assert_ne!(current[0]["items"], original[0]["items"]);
  assert_eq!(current[0]["more"], original[0]["more"]);
}

#[test]
fn test_anonymous_membership_order_does_not_change_module_identity() {
  let first = json!({"data":{"items":[{"id":"1"},{"id":"2"}]}});
  let second = json!({"data":{"items":[{"id":"2"},{"id":"1"}]}});
  let before = label_release_shelves(9, &json!({"releases":[first]}));
  let after = label_release_shelves(9, &json!({"releases":[second]}));
  assert_eq!(before[0]["id"], after[0]["id"]);
  assert_eq!(before[0]["items"][0]["ref"], "album:1");
  assert_eq!(after[0]["items"][0]["ref"], "album:2");
}

#[test]
fn test_named_fallback_keeps_its_identity_across_metadata_spacing_and_new_records() {
  let first = json!({"title":"  New   Releases ","data":{"items":[{"id":"1"}]}});
  let second = json!({"title":"new releases","data":{"items":[{"id":"2"}]}});
  let before = label_release_shelves(9, &json!({"releases":[first]}));
  let after = label_release_shelves(9, &json!({"releases":[second]}));
  assert_eq!(before[0]["id"], after[0]["id"]);
  assert_ne!(before[0]["items"], after[0]["items"]);
}

#[test]
fn test_award_routes_keep_explicit_sections_and_reject_invalid_offsets() {
  assert_eq!(award_route(&["award", "press-7"]).unwrap(), ("press-7", None));
  assert_eq!(award_route(&["award", "7", "albums", "14"]).unwrap(), ("7", Some(14)));
  for parts in [
    vec!["award", ""],
    vec!["award", "7", "tracks", "0"],
    vec!["award", "7", "albums", "bad"],
    vec!["award", "7", "albums", "-1"],
    vec!["award", "7", "albums", "4294967295"],
    vec!["award", "7", "albums", "0", "extra"],
  ] {
    assert_eq!(award_route(&parts).unwrap_err().code, -32602);
  }
}

#[test]
fn test_award_header_keeps_identity_and_does_not_mask_a_failed_metadata_response() {
  let page = json!({"id":"7","name":" Press choice ","magazine":{"name":"Review","image":"https://images.test/press.jpg"}});
  let header = award_header("7", &page).unwrap();
  assert_eq!(header["kind"], "award");
  assert_eq!(header["ref"], "award:7");
  assert_eq!(header["title"], "Press choice");
  assert_eq!(header["image"], "https://images.test/press.jpg");
  assert_eq!(header["image_fallback"], "award");
  let empty = award_header("7", &json!({"id":"7","name":"Press choice"})).unwrap();
  assert!(empty["image"].is_null());
  assert_eq!(empty["image_fallback"], "award");
  assert!(award_header("8", &page).is_err());
  assert!(award_header("7", &json!({"status":"error","message":"Forbidden"})).is_err());
}

#[test]
fn test_award_grid_uses_album_projection_and_advances_by_received_rows() {
  let first = award_albums_page(
    "7",
    0,
    &json!({
      "offset":0,"total":5,"items":[{"id":"abc","title":"Record","artist":{"id":9,"name":"Artist"}}]
    }),
  )
  .unwrap();
  let later = award_albums_page(
    "7",
    1,
    &json!({
      "offset":1,"total":5,"items":[{"id":"def","title":"Other"}]
    }),
  )
  .unwrap();
  assert_eq!(first["items"][0]["kind"], "album");
  assert_eq!(first["items"][0]["ref"], "album:abc");
  assert_eq!(first["items"][0]["artist"], "Artist");
  assert_eq!(first["next"], "award:7:albums:1");
  assert_eq!(later["next"], "award:7:albums:2");
  assert_eq!(first["tabs"], later["tabs"]);
  assert_eq!(first["tabs"][0]["ref"], "award:7:albums:0");
}

#[test]
fn test_award_grid_finishes_only_at_the_total_and_rejects_nonadvancing_pages() {
  let last =
    award_albums_page("7", 1, &json!({"offset":1,"total":2,"items":[{"id":"last"}]})).unwrap();
  assert!(last["next"].is_null());
  assert!(
    award_albums_page("7", 0, &json!({"offset":0,"total":0,"items":[]})).unwrap()["next"].is_null()
  );
  for page in [
    json!({"offset":0,"total":1,"items":[]}),
    json!({"offset":1,"total":2,"items":[{"id":"wrong-offset"}]}),
    json!({"offset":0,"total":0,"items":[{"id":"beyond-total"}]}),
    json!({"offset":0,"items":[]}),
  ] {
    assert!(award_albums_page("7", 0, &page).is_err());
  }
}
