use super::*;
#[test]
fn test_start_is_form_encoded_and_end_is_json_without_a_request_signature() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let http = reqwest::Client::new();
  let start = request(
    &http,
    "https://example.test/start",
    true,
    &[json!({"track_id":42,"date":123,"user_id":7,"format_id":27})],
    "fixture/1.2.3",
  )
  .build()
  .unwrap();
  assert_eq!(start.method(), reqwest::Method::POST);
  assert_eq!(start.headers()["content-type"], "application/x-www-form-urlencoded");
  let body = std::str::from_utf8(start.body().unwrap().as_bytes().unwrap()).unwrap();
  assert!(body.starts_with("events=%5B"));
  assert!(start.url().query().is_none());
  let end = request(
    &http,
    "https://example.test/end",
    false,
    &[json!({"blob":"opaque-fixture","duration":12})],
    "fixture/1.2.3",
  )
  .build()
  .unwrap();
  assert_eq!(end.headers()["content-type"], "application/json");
  let body: Value = serde_json::from_slice(end.body().unwrap().as_bytes().unwrap()).unwrap();
  assert_eq!(body["events"][0]["duration"], 12);
  assert_eq!(body["renderer_context"]["software_version"], "fixture/1.2.3");
}
