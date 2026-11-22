use super::*;

#[test]
fn test_album_unavailable_recognises_only_the_exact_get_album_404() {
  assert!(
    ApiError::ApiResponse("Album old-id not found (404)".into()).is_album_unavailable("old-id")
  );
  assert!(
    !ApiError::ApiResponse("get_album(old-id) status 500".into()).is_album_unavailable("old-id")
  );
  assert!(!ApiError::ApiResponse("Album another-id not found (404)".into())
    .is_album_unavailable("old-id"));
  assert!(!ApiError::OfflineMode.is_album_unavailable("old-id"));
}
