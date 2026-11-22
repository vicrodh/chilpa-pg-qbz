use super::*;

#[test]
fn test_transport_diagnostic_never_echoes_signed_url() {
  let marker = "https://cdn.example/audio.flac?jwt=secret&request_sig=signed";
  let diagnostic = safe_transport_diagnostic(marker);
  assert_eq!(diagnostic, "HTTP transport request failed");
  assert!(!diagnostic.contains(marker));
  assert!(!diagnostic.contains("secret"));
}

/// The CMAF path: a real reqwest failure (connection refused) whose
/// Display embeds the signed URL must classify without it.
#[tokio::test]
async fn test_classify_reqwest_never_echoes_the_request_url() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let port = {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    probe.local_addr().unwrap().port()
  }; // dropped: nothing listens on `port` now
  let url = format!("http://127.0.0.1:{port}/file?eid=1&hmac=SIGNEDSECRET&etsp=1");
  let err = reqwest::Client::new().get(&url).send().await.unwrap_err();
  assert!(
    err.to_string().contains("SIGNEDSECRET"),
    "precondition: reqwest's Display embeds the url: {err}"
  );

  assert_eq!(describe_reqwest_error(&err), "HTTP transport connection failed");
  let classified = crate::retry::classify_reqwest(&err, "fetch").to_string();
  assert_eq!(classified, "fetch: HTTP transport connection failed");
  assert!(!classified.contains("SIGNEDSECRET"));
  assert!(!classified.contains("127.0.0.1"));
}
