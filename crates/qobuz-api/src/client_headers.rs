use super::client::*;
impl QobuzClient {
  // === Header helpers ===

  /// Build standard API headers.
  /// Always includes X-App-Id. Includes X-User-Auth-Token when logged in.
  pub(crate) async fn api_headers(&self) -> Result<reqwest::header::HeaderMap> {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();

    let app_id = self.app_id().await?;
    headers.insert("X-App-Id", HeaderValue::from_str(&app_id).map_err(|_| ApiError::InvalidAppId)?);

    if let Ok(token) = self.auth_token().await {
      if let Ok(val) = HeaderValue::from_str(&token) {
        headers.insert("X-User-Auth-Token", val);
      }
    }

    Ok(headers)
  }

  /// Build headers that REQUIRE authentication. Fails if not logged in.
  pub(crate) async fn authenticated_headers(&self) -> Result<reqwest::header::HeaderMap> {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();

    let app_id = self.app_id().await?;
    headers.insert("X-App-Id", HeaderValue::from_str(&app_id).map_err(|_| ApiError::InvalidAppId)?);

    let token = self.auth_token().await?;
    headers.insert(
      "X-User-Auth-Token",
      HeaderValue::from_str(&token)
        .map_err(|_| ApiError::AuthenticationError("Invalid auth token format".into()))?,
    );

    Ok(headers)
  }

  /// Catalogue read through the official browser account.
  pub(crate) async fn signed_get(
    &self,
    url: &str,
    _method_name: &str,
    params: &[(&str, String)],
  ) -> Result<reqwest::Response> {
    Ok(self.http()?.get(url).headers(self.api_headers().await?).query(params).send().await?)
  }

  pub(crate) async fn signed_get_auth(
    &self,
    url: &str,
    _method_name: &str,
    params: &[(&str, String)],
  ) -> Result<reqwest::Response> {
    Ok(
      self
        .http()?
        .get(url)
        .headers(self.authenticated_headers().await?)
        .query(params)
        .send()
        .await?,
    )
  }
}
