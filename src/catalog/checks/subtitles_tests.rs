use super::*;

const WORDS: &str = r#"{"album_id":"a1","track_id":7,"translation_langs":["es"],
        "publishers":[{"copyright":"Example Publishing","zones":["WW"]}],
        "writers":"First Writer, Second Writer",
        "original":{"type":"wsync","lang":"en","lines":[
            {"line":"Placeholder one two","start":1000,"end":2500,"words":[
                {"word":"Placeholder ","start":1000,"end":1500},
                {"word":"one","start":1500,"end":2000},
                {"word":" two","start":2000,"end":2500}]},
            {"line":"","words":[]},
            {"line":"Placeholder three","start":9000,"end":10000,"words":[
                {"word":"Placeholder","start":9000,"end":9500},
                {"word":"three","start":9500,"end":10000}]}]}}"#;

const PLAIN: &str = r#"{"album_id":"a1","track_id":7,"writers":"Only Writer",
        "original":{"type":"plain","lang":"en","lines":[
            {"line":"Placeholder one"},{"line":""},"Placeholder two",{"line":"  Placeholder three  "}]}}"#;

const TRANSLATED: &str = r#"{"album_id":"a1","track_id":7,
        "translation":{"type":"lsync","lang":"es","lines":[
            {"line":"Marcador uno dos","start":1000,"end":2500},
            {"line":"","words":[]},
            {"line":"Marcador tres","start":9000,"end":10000}]}}"#;

fn read(text: &str) -> QobuzLyricsDocument {
  serde_json::from_str(text).unwrap()
}

#[test]
fn test_word_segments() {
  let track = json!({"duration_ms": 215000, "isrc": "QZXXX0000001", "title": "ignored"});
  let found = answer(&read(WORDS), None, None, Some(&track));
  assert_eq!(found["status"], "found");
  assert_eq!(found["lyrics"]["format"], "document");
  let document = &found["lyrics"]["document"];
  assert_eq!(document["schema"], 1);
  assert_eq!(document["timing"], "word");
  assert_eq!(document["language"], "en");
  let line = &document["lines"][0];
  assert_eq!((line["start_ms"].as_u64(), line["end_ms"].as_u64()), (Some(1000), Some(2500)));
  assert_eq!(line["text"], "Placeholder one two");
  let words: Vec<(&str, u64, u64, bool)> = line["segments"]
    .as_array()
    .unwrap()
    .iter()
    .map(|s| {
      (
        s["text"].as_str().unwrap(),
        s["start_ms"].as_u64().unwrap(),
        s["end_ms"].as_u64().unwrap(),
        s["space_after"].as_bool().unwrap(),
      )
    })
    .collect();
  assert_eq!(
    words,
    vec![("Placeholder", 1000, 1500, true), ("one", 1500, 2000, true), ("two", 2000, 2500, false)]
  );
  assert_eq!(found["fingerprint"], json!({"duration_ms": 215000, "isrc": "QZXXX0000001"}));
  assert_eq!(
    document["credits"],
    json!([{"name": "First Writer", "role": "songwriter"}, {"name": "Second Writer", "role": "songwriter"}])
  );
  assert!(document.get("layers").is_none());
}

#[test]
fn test_silent_lines() {
  let found = answer(&read(WORDS), None, None, None);
  let lines = found["lyrics"]["document"]["lines"].as_array().unwrap();
  assert_eq!(lines.len(), 2);
  assert_eq!(lines[0]["end_ms"], 2500);
  assert_eq!(lines[1]["start_ms"], 9000);
  assert!(found.get("fingerprint").is_none(), "nothing is invented about the track");
}

#[test]
fn test_plain_order() {
  let found = answer(&read(PLAIN), None, None, None);
  let document = &found["lyrics"]["document"];
  assert_eq!(document["timing"], "unsynced");
  let lines = document["lines"].as_array().unwrap();
  let texts: Vec<&str> = lines.iter().map(|l| l["text"].as_str().unwrap()).collect();
  assert_eq!(texts, vec!["Placeholder one", "Placeholder two", "Placeholder three"]);
  assert!(lines.iter().all(|l| l.get("start_ms").is_none() && l.get("segments").is_none()));
}

#[test]
fn test_line_timing() {
  let document =
    read(r#"{"original":{"type":"lsync","lines":[{"line":"Placeholder","start":500,"end":900}]}}"#);
  let found = answer(&document, None, None, None);
  assert_eq!(found["lyrics"]["document"]["timing"], "line");
  assert!(found["lyrics"]["document"]["lines"][0].get("segments").is_none());
  assert!(found["lyrics"]["document"].get("language").is_none());
}

#[test]
fn test_translation_alignment() {
  let translated = read(TRANSLATED);
  let found = answer(&read(WORDS), translated.translation.as_ref(), Some("es"), None);
  let layers = found["lyrics"]["document"]["layers"].as_array().unwrap();
  assert_eq!(layers.len(), 1);
  assert_eq!(layers[0]["kind"], "translation");
  assert_eq!(layers[0]["language"], "es");
  assert_eq!(
    layers[0]["lines"],
    json!([{"line": 0, "text": "Marcador uno dos"}, {"line": 1, "text": "Marcador tres"}])
  );

  let same = read(WORDS);
  let found = answer(&read(WORDS), same.original.as_ref(), Some("en"), None);
  assert!(
    found["lyrics"]["document"].get("layers").is_none(),
    "the lyrics' own language is not a translation"
  );
}

#[test]
fn test_empty_lyrics() {
  assert_eq!(answer(&read(r#"{"track_id":7}"#), None, None, None), json!({"status": "none"}));
  assert_eq!(
    answer(&read(TRANSLATED), None, None, None),
    json!({"status": "none"}),
    "a translation alone is not the lyrics"
  );
  let empty = read(r#"{"original":{"type":"wsync","lines":[{"line":"","words":[]}]}}"#);
  assert_eq!(answer(&empty, None, None, None), json!({"status": "none"}));
}

#[test]
fn test_lookup_statuses() {
  let miss =
    r#"{"status":"error","code":404,"message":"Lyrics are not available for this track."}"#;
  assert_eq!(address(404, None, miss).ok(), Some(None));
  assert_eq!(
    address(200, None, r#"{"track_id":7,"album_id":"a1"}"#).ok(),
    Some(None),
    "no address, no lyrics"
  );
  assert_eq!(
    address(
      200,
      None,
      r#"{"track_id":7,"lyrics_url":"https://d0.cloudfront.net/v1/7.json?Signature=x"}"#
    )
    .ok(),
    Some(Some("https://d0.cloudfront.net/v1/7.json?Signature=x".to_string()))
  );
  for status in [401, 403, 500, 503] {
    let failure = address(status, None, "{}").err().expect("a failure");
    assert_eq!(failure.code, -32000);
    assert!(failure.data.is_none());
  }
  assert!(address(200, None, "<html>").is_err(), "an answer that cannot be read is not \"none\"");

  assert!(
    matches!(document(403, None, "AccessDenied"), Ok(None)),
    "the CDN refuses a translation that does not exist"
  );
  assert!(matches!(document(404, None, ""), Ok(None)));
  assert!(document(502, None, "").is_err());
  assert!(document(200, None, "not json").is_err());
  assert!(matches!(document(200, None, WORDS), Ok(Some(_))));
}

#[test]
fn test_retry_delay() {
  let failure = address(429, Some(7), "").err().expect("a failure");
  assert_eq!(failure.data, Some(json!({"retry_after_ms": 7000})));
  let failure = address(429, None, "").err().expect("a failure");
  assert_eq!(
    failure.data,
    Some(json!({"retry_after_ms": 1000})),
    "without a number it is one second"
  );
  let failure = document(429, Some(86_400), "").err().expect("a failure");
  assert_eq!(
    failure.data,
    Some(json!({"retry_after_ms": 3_600_000})),
    "a long wait is capped at an hour"
  );
}

#[test]
fn test_cdn_hosts() {
  assert!(from_the_cdn("https://d12eggcejhfkay.cloudfront.net/v1.3/a/b/c/7.json?Expires=1"));
  assert!(!from_the_cdn("http://d12eggcejhfkay.cloudfront.net/v1.3/7.json"));
  assert!(!from_the_cdn("https://example.com/cloudfront.net/7.json"));
  assert!(!from_the_cdn("not an address"));
}

#[test]
fn test_translation_codes() {
  assert_eq!(language_of(&json!({"ref": "track:7", "language": " ES "})), Some("es".to_string()));
  assert_eq!(language_of(&json!({"ref": "track:7", "language": "spanish"})), None);
  assert_eq!(language_of(&json!({"ref": "track:7"})), None);
}
