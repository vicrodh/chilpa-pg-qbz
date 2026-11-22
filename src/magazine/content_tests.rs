use super::*;

fn story(blocks: Vec<Value>) -> Value {
  json!({"_id":"STORY1","headlines":{"basic":"An &amp; article"},
    "subheadlines":{"basic":"<b>Summary</b>"},"display_date":"2026-11-16T10:00:00Z",
    "credits":{"by":[{"additional_properties":{"original":{"byline":"An author"}}}]},
    "content_elements":blocks})
}

fn picture(id: u32) -> Value {
  json!({"type":"image","additional_properties":{
    "originalUrl":format!("https://cloudfront-eu-central-1.images.arcpublishing.com/qobuz/{id}.jpg"),
    "fullSizeResizeUrl":format!("/resizer/v2/{id}.jpg?auth={}","a".repeat(64))},
    "width":600,"height":800,"caption":"<i>Caption</i>","subtitle":"A picture"})
}

#[test]
fn cms_markup_becomes_plain_text_and_active_content_is_discarded() {
  assert_eq!(plain("<p>Caf&eacute; &amp; &#x266b;</p><p>Next<br>line</p><script>bad()</script><iframe src='other'>hidden</iframe>"),"Café & ♫\nNext\nline");
  assert_eq!(
    plain("<!-- hidden -->Visible <style>bad</style> &lt;literal&gt;"),
    "Visible <literal>"
  );
  let wire = story(vec![
    json!({"type":"header","level":99,"content":"<b>Heading</b>"}),
    json!({"type":"text","content":"<a href='javascript:bad()'>Readable</a>"}),
    json!({"type":"quote","content_elements":[{"content":"<i>Quote</i>"}]}),
    json!({"type":"list","list_type":"ordered","items":["First",{"content":"<b>Second</b>"}]}),
    picture(1),
    json!({"type":"custom_embed","embed":{"html":"<iframe>bad</iframe>"}}),
  ]);
  let page = article(&wire, "STORY1").unwrap();
  assert_eq!(page["title"], "An & article");
  assert_eq!(page["article"]["authors"], json!(["An author"]));
  let blocks = page["article"]["blocks"].as_array().unwrap();
  assert_eq!(blocks.len(), 5);
  assert_eq!(blocks[0]["level"], 6);
  assert_eq!(blocks[1]["text"], "Readable");
  assert_eq!(blocks[3]["items"], json!(["First", "Second"]));
  assert_eq!(blocks[4]["caption"], "Caption");
  let encoded = serde_json::to_string(&page).unwrap();
  assert!(!encoded.contains("javascript"));
  assert!(!encoded.contains("iframe"));
}

#[test]
fn cover_counts_towards_the_image_budget_and_duplicates_do_not_load_twice() {
  let mut wire = story(vec![
    json!({"type":"text","content":"Text"}),
    json!({"type":"gallery","content_elements":(0..20).map(picture).collect::<Vec<_>>()}),
  ]);
  wire["promo_items"] = json!({"basic":picture(0)});
  let page = article(&wire, "STORY1").unwrap();
  let blocks = page["article"]["blocks"].as_array().unwrap();
  assert_eq!(blocks.iter().filter(|block| block["type"] == "image").count(), 7);
  assert!(page["article"]["image"].is_string());
  assert_eq!(blocks.iter().filter(|block| block["type"] == "paragraph").count(), 1);
}

#[test]
fn malformed_identity_empty_content_and_excessive_text_do_not_publish_partial_articles() {
  let wire = story(vec![json!({"type":"text","content":"Hello"})]);
  assert!(article(&wire, "OTHER").is_err());
  assert!(article(
    &story(vec![json!({"type":"raw_html","content":"<iframe>bad</iframe>"})]),
    "STORY1"
  )
  .is_err());
  let too_many = story((0..257).map(|_| json!({"type":"text","content":"paragraph"})).collect());
  assert!(article(&too_many, "STORY1").is_err());
  let huge = story((0..9).map(|_| json!({"type":"text","content":"x".repeat(64*1024)})).collect());
  assert!(article(&huge, "STORY1").is_err());
  assert!(article(&story(vec![json!({"type":"text","content":"x".repeat(64*1024+1)})]), "STORY1")
    .is_err());
}

#[test]
fn image_urls_cannot_carry_credentials_or_point_to_another_origin() {
  for address in [
    "file:///etc/passwd",
    "https://token@static.qobuz.com/a.jpg",
    "https://localhost/a.jpg",
    "https://static.qobuz.com/a.jpg?token=private",
    "https://static.qobuz.com:444/a.jpg",
  ] {
    assert!(public_image(&json!({"url":address})).is_none());
  }
}

#[test]
fn home_deduplicates_articles_and_navigation_is_localized_and_bounded() {
  let wire = story(vec![]);
  assert_eq!(
    home(&json!([{"stories":[wire.clone()]},{"stories":[wire.clone()]}])).unwrap().len(),
    1
  );
  assert!(teaser(&json!({"_id":"../bad","headlines":{"basic":"Bad"}})).is_none());
  let nav = json!([{"_id":"/news","name_tr":{"name_en":"Articles","name_es":"Actualidad"},
    "children":[{"_id":"/news/tests","name_tr":{"name_en":"Tests"}}]},
    {"_id":"/../bad","name_tr":{"name_en":"Unsafe"}}]);
  let options = sections(&nav, "es").unwrap();
  assert_eq!(options.len(), 2);
  assert_eq!(options[0]["label"], "Actualidad");
  assert_eq!(options[1]["label"], "Tests");
  assert!(home(&json!({"error":"upstream failure"})).is_err());
  assert!(home(&json!([{"error":"upstream failure"}])).is_err());
  assert!(sections(&json!({"error":"upstream failure"}), "en").is_err());
}

#[test]
fn editorial_album_lists_reviews_and_specifications_keep_their_readable_content() {
  let wire = story(vec![
    json!({"type":"custom_embed","subtype":"qobuz-albums","embed":{"config":{
      "title":"Recommended","subtitle":"A selection","objects":[{"title":"An album","artist":{"name":"An artist"},
      "image":{"large":"https://static.qobuz.com/images/covers/a_600.jpg"}}]}}}),
    json!({"type":"custom_embed","subtype":"qobuz-pros-cons","embed":{"config":{
      "title":"Review","desc":"<b>Summary</b>","p":["Clear sound"],"c":["Large enclosure"],"rating":"4.5"}}}),
    json!({"type":"custom_embed","subtype":"qobuz-features","embed":{"config":{
      "title":"Specifications","el":[{"k":"Inputs","v":4},{"k":"Mode","v":"USB"}]}}}),
    json!({"type":"custom_embed","subtype":"unknown","embed":{"config":{"html":"<iframe>hidden</iframe>"}}}),
  ]);
  let page = article(&wire, "STORY1").unwrap();
  let encoded = serde_json::to_string(&page).unwrap();
  for expected in ["An album — An artist", "Clear sound", "Large enclosure", "4.5 / 5", "Inputs: 4"]
  {
    assert!(encoded.contains(expected), "Missing editorial text: {expected}");
  }
  assert!(!encoded.contains("iframe"));
  assert_eq!(
    page["article"]["blocks"]
      .as_array()
      .unwrap()
      .iter()
      .filter(|block| block["type"] == "image")
      .count(),
    1
  );
}

#[test]
fn lists_cannot_create_an_unbounded_number_of_native_rows() {
  let wire = story(vec![json!({"type":"list","items":vec!["item";257]})]);
  assert!(article(&wire, "STORY1").is_err());
}

#[test]
fn the_list_item_limit_applies_to_the_whole_document() {
  let wire = story(vec![
    json!({"type":"list","items":vec!["item";200]}),
    json!({"type":"list","items":vec!["other";200]}),
  ]);
  assert!(article(&wire, "STORY1").is_err());
}

#[test]
fn control_characters_cannot_invalidate_a_plain_text_article() {
  assert_eq!(plain("Readable\0\u{1}<b> text</b>"), "Readable text");
}
