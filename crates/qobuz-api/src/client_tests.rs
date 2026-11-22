use super::client::*;
fn available_master(format_id: u32) -> StreamUrl {
  StreamUrl {
    blob: None,
    duration: None,
    url: "https://cdn.example.test/audio".into(),
    format_id,
    mime_type: "audio/flac".into(),
    sampling_rate: if format_id == 6 { 44.1 } else { 96.0 },
    bit_depth: Some(if format_id == 6 { 16 } else { 24 }),
    track_id: 7,
    restrictions: Vec::new(),
    sample: false,
  }
}

#[tokio::test]
async fn test_successful_request_keeps_available_format_without_extra_attempts() {
  for format_id in [6, 7] {
    let mut attempts = Vec::new();
    let (url, actual) =
      QobuzClient::resolve_stream_url_with_fallback(7, Quality::UltraHiRes, |quality| {
        attempts.push(quality);
        std::future::ready(Ok(available_master(format_id)))
      })
      .await
      .unwrap();
    assert_eq!(attempts, vec![Quality::UltraHiRes]);
    assert_eq!(actual, Quality::UltraHiRes);
    assert_eq!(url.format_id, format_id);
  }
}

#[tokio::test]
async fn test_successful_request_reports_actual_fallback_not_initial_preference() {
  let mut attempts = Vec::new();
  let (url, actual) =
    QobuzClient::resolve_stream_url_with_fallback(7, Quality::UltraHiRes, |quality| {
      attempts.push(quality);
      let mut url = available_master(7);
      if quality == Quality::UltraHiRes {
        url.restrictions.push(qobuz_models::StreamRestriction {
          code: "FormatRestrictedByFormatAvailability".into(),
        });
      }
      std::future::ready(Ok(url))
    })
    .await
    .unwrap();
  assert_eq!(attempts, vec![Quality::UltraHiRes, Quality::HiRes]);
  assert_eq!(actual, Quality::HiRes);
  assert_eq!(url.format_id, 7);
}

#[tokio::test]
async fn test_successful_request_aborts_auth_failure_without_lower_quality_requests() {
  for error in [
    ApiError::Forbidden("fixture".into()),
    ApiError::ForbiddenCircuitOpen(30),
    ApiError::AuthenticationError("fixture".into()),
    ApiError::InvalidAppSecret,
  ] {
    let expected = std::mem::discriminant(&error);
    let mut error = Some(error);
    let mut attempts = Vec::new();
    let result = QobuzClient::resolve_stream_url_with_fallback(7, Quality::UltraHiRes, |quality| {
      attempts.push(quality);
      std::future::ready(Err(error.take().expect("must abort after first failure")))
    })
    .await;
    assert_eq!(std::mem::discriminant(&result.unwrap_err()), expected);
    assert_eq!(attempts, vec![Quality::UltraHiRes]);
  }
}

#[test]
fn test_remote_body_diagnostic_never_echoes_payload() {
  let marker = "jwt=secret&request_sig=signed-value";
  let diagnostic = redacted_body_diagnostic(marker);
  assert_eq!(diagnostic, " : <response body omitted>");
  assert!(!diagnostic.contains(marker));
  assert!(!diagnostic.contains("secret"));
  assert_eq!(redacted_body_diagnostic("  \n"), " : <empty body>");
}
