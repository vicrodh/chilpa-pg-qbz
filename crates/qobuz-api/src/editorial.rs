use crate::client::*;

const BASE: &str = "https://qobuz-qobuz-prod.web.arc-cdn.net/arc/outboundfeeds/";

pub enum EditorialPage<'a> {
  Home,
  Navigation,
  Section { slug: &'a str, offset: u32, limit: u32 },
  Story(&'a str),
}

pub fn valid_store(store: &str) -> bool {
  store.len() <= 16
    && (2..=3).contains(&store.split('-').count())
    && store.split('-').all(|part| {
      (2..=3).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_lowercase())
    })
}

pub fn valid_slug(slug: &str) -> bool {
  slug.len() <= 128
    && slug.starts_with('/')
    && slug.split('/').skip(1).all(|part| {
      !part.is_empty()
        && part
          .bytes()
          .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

pub fn valid_story(id: &str) -> bool {
  !id.is_empty()
    && id.len() <= 128
    && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn route(store: &str, page: EditorialPage<'_>) -> Result<(String, Vec<(&'static str, String)>)> {
  if !valid_store(store) {
    return Err(ApiError::ApiResponse("Invalid magazine store".into()));
  }
  let mut query = Vec::new();
  let route = match page {
    EditorialPage::Home => format!("{store}/homepage/"),
    EditorialPage::Navigation => format!("navigation/{store}/"),
    EditorialPage::Story(id) if valid_story(id) => format!("story/{store}/{id}/"),
    EditorialPage::Section { slug, offset, limit }
      if valid_slug(slug) && offset <= 10000 && (1..=100).contains(&limit) =>
    {
      query = vec![("from", offset.to_string()), ("size", limit.to_string())];
      format!("{store}/section{slug}/")
    }
    _ => return Err(ApiError::ApiResponse("Invalid magazine reference".into())),
  };
  Ok((format!("{BASE}{route}"), query))
}

impl QobuzClient {
  /// The editorial store is supplied by the account, not inferred from locale.
  pub async fn editorial_store(&self) -> Result<String> {
    let response = self
      .http()?
      .get(endpoints::build_url(paths::USER_GET))
      .headers(self.authenticated_headers().await?)
      .send()
      .await?;
    let profile = crate::library_pages::read_response(response).await?;
    let store = profile
      .pointer("/user/store")
      .or_else(|| profile.get("store"))
      .and_then(Value::as_str)
      .unwrap_or_default()
      .to_ascii_lowercase();
    if !valid_store(&store) {
      return Err(ApiError::ApiResponse("The account has no magazine store".into()));
    }
    Ok(store)
  }

  /// Public editorial feeds receive no account headers or cookies.
  pub async fn editorial_page(&self, store: &str, page: EditorialPage<'_>) -> Result<Value> {
    let (url, query) = route(store, page)?;
    let response = self.http()?.get(url).query(&query).send().await?;
    crate::library_pages::read_response(response).await
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn editorial_routes_stay_on_the_public_feed() {
    let (url, query) =
      route("fr-fr", EditorialPage::Section { slug: "/hi-fi/tests", offset: 20, limit: 20 })
        .unwrap();
    assert_eq!(url, format!("{BASE}fr-fr/section/hi-fi/tests/"));
    assert_eq!(query, vec![("from", "20".into()), ("size", "20".into())]);
    assert!(route("fr", EditorialPage::Home).is_err());
    for id in ["../other", "a?token=secret", "https://other.test", ""] {
      assert!(route("us-en", EditorialPage::Story(id)).is_err());
    }
    for slug in ["//other", "/a/../b", "/a?x=y", "/a/"] {
      assert!(route("us-en", EditorialPage::Section { slug, offset: 0, limit: 20 }).is_err());
    }
    assert!(
      route("us-en", EditorialPage::Section { slug: "/news", offset: 0, limit: 101 }).is_err()
    );
  }
}
