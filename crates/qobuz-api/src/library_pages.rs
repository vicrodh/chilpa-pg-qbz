use crate::client::*;
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashSet};

pub const MAX_PAGE_SIZE: u32 = 500;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const OVERSIZED_RESPONSE: &str = "Invalid library page: response exceeds 4 MiB";

pub fn response_too_large(error: &ApiError) -> bool {
  matches!(error, ApiError::ApiResponse(message) if message == OVERSIZED_RESPONSE)
}

#[derive(Debug)]
pub struct LibraryPage {
  pub items: Vec<Value>,
  pub next_offset: Option<u64>,
  pub total: Option<u64>,
}

/// Collection revision dates in Unix milliseconds; an explicit null remains None.
pub type LibraryRevisions = BTreeMap<String, Option<i64>>;

fn invalid(message: &str) -> ApiError {
  ApiError::ApiResponse(format!("Invalid library page: {message}"))
}

fn page_size(limit: u32) -> Result<()> {
  if limit == 0 || limit > MAX_PAGE_SIZE {
    return Err(invalid("page size must be between 1 and 500"));
  }
  Ok(())
}

fn status_result(status: StatusCode, retry_after: Option<&str>) -> Result<()> {
  match status {
    StatusCode::UNAUTHORIZED => Err(ApiError::AuthenticationError("Library needs sign-in".into())),
    StatusCode::FORBIDDEN => Err(ApiError::Forbidden(String::new())),
    StatusCode::TOO_MANY_REQUESTS => {
      Err(ApiError::RateLimited(retry_after.and_then(|text| text.parse().ok()).unwrap_or(30)))
    }
    status if status.is_server_error() => Err(ApiError::ServerError(status.as_u16())),
    status if !status.is_success() => {
      Err(ApiError::ApiResponse(format!("Library returned HTTP {}", status.as_u16())))
    }
    _ => Ok(()),
  }
}

fn append_chunk(buffer: &mut Vec<u8>, chunk: &[u8]) -> Result<()> {
  if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(buffer.len()) {
    return Err(invalid("response exceeds 4 MiB"));
  }
  buffer.extend_from_slice(chunk);
  Ok(())
}

pub(crate) async fn read_response(response: reqwest::Response) -> Result<Value> {
  status_result(
    response.status(),
    response.headers().get("retry-after").and_then(|v| v.to_str().ok()),
  )?;
  if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
    return Err(invalid("response exceeds 4 MiB"));
  }
  let mut buffer = Vec::new();
  let mut chunks = response.bytes_stream();
  while let Some(chunk) = chunks.next().await {
    append_chunk(&mut buffer, &chunk?)?;
  }
  Ok(serde_json::from_slice(&buffer)?)
}

fn optional_count(value: Option<&Value>, name: &str) -> Result<Option<u64>> {
  match value {
    None | Some(Value::Null) => Ok(None),
    Some(value) => value.as_u64().map(Some).ok_or_else(|| invalid(name)),
  }
}

fn service_result(body: &Value) -> Result<()> {
  if body.get("status").is_some_and(|value| !value.is_null() && value != "success")
    || body.get("error").is_some_and(|value| !value.is_null())
  {
    return Err(invalid("service returned an error envelope"));
  }
  Ok(())
}

fn revision_millis(value: &Value) -> Result<Option<i64>> {
  if value.is_null() {
    return Ok(None);
  }
  if let Some(seconds) = value.as_i64().filter(|seconds| *seconds >= 0) {
    return seconds.checked_mul(1000).map(Some).ok_or_else(|| invalid("revision date overflow"));
  }
  if let Some(seconds) = value.as_f64() {
    let millis = seconds * 1000.0;
    if millis.is_finite() && millis >= 0.0 && millis < i64::MAX as f64 {
      return Ok(Some(millis.trunc() as i64));
    }
  }
  if let Some(text) = value.as_str() {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
      return (date.timestamp_millis() >= 0)
        .then_some(Some(date.timestamp_millis()))
        .ok_or_else(|| invalid("negative revision date"));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
      let millis = date.and_hms_opt(0, 0, 0).expect("midnight").and_utc().timestamp_millis();
      return (millis >= 0)
        .then_some(Some(millis))
        .ok_or_else(|| invalid("negative revision date"));
    }
  }
  Err(invalid("invalid revision date"))
}

fn parse_revisions(body: Value) -> Result<LibraryRevisions> {
  service_result(&body)?;
  let updates = body
    .get("last_update")
    .and_then(Value::as_object)
    .ok_or_else(|| invalid("missing revision object"))?;
  [
    ("favorite-albums", "favorite_album"),
    ("favorite-tracks", "favorite_track"),
    ("followed-artists", "favorite_artist"),
    ("followed-labels", "favorite_label"),
    ("followed-awards", "favorite_award"),
    ("purchased-albums", "purchase"),
    ("purchased-tracks", "purchase"),
    ("playlists", "playlist"),
  ]
  .into_iter()
  .map(|(source, key)| {
    let value = updates.get(key).ok_or_else(|| invalid("missing collection revision"))?;
    Ok((source.to_owned(), revision_millis(value)?))
  })
  .collect()
}

impl LibraryPage {
  fn parse(mut body: Value, kind: &str, limit: u32, offset: u64) -> Result<Self> {
    page_size(limit)?;
    service_result(&body)?;
    let page = body
      .get_mut(kind)
      .and_then(Value::as_object_mut)
      .ok_or_else(|| invalid("missing collection"))?;
    if optional_count(page.get("offset"), "invalid offset")?.is_some_and(|actual| actual != offset)
    {
      return Err(invalid("offset did not advance as requested"));
    }
    let total = optional_count(page.get("total"), "invalid total")?;
    if optional_count(page.get("limit"), "invalid limit")? == Some(0) {
      return Err(invalid("zero server page size"));
    }
    let has_more = match page.get("has_more").or_else(|| page.get("hasMore")) {
      None | Some(Value::Null) => None,
      Some(value) => Some(value.as_bool().ok_or_else(|| invalid("invalid continuation flag"))?),
    };
    let items = match page.remove("items") {
      Some(Value::Array(items)) => items,
      _ => return Err(invalid("missing item array")),
    };
    if items.len() > limit as usize {
      return Err(invalid("service exceeded the requested item limit"));
    }
    // The service repeats an item within a page in some accounts: the first
    // copy is kept, and the offset still advances by every row it sent.
    let received = items.len() as u64;
    let mut identities = HashSet::with_capacity(items.len());
    let mut unique = Vec::with_capacity(items.len());
    for item in items {
      let id = match item.get("id") {
        Some(Value::String(text)) if !text.is_empty() && text.len() <= 256 => text.clone(),
        Some(Value::Number(number)) if number.as_u64().is_some_and(|id| id != 0) => {
          number.to_string()
        }
        _ => return Err(invalid("item has no valid identity")),
      };
      if identities.insert(id) {
        unique.push(item);
      }
    }
    let consumed = offset.checked_add(received).ok_or_else(|| invalid("offset overflow"))?;
    // Any signal that the collection ended ends it. A reported total may
    // count rows the service never sends, and an empty page means nothing
    // follows it.
    let more = received > 0
      && total.is_none_or(|total| consumed < total)
      && has_more.unwrap_or(true);
    Ok(Self { items: unique, next_offset: more.then_some(consumed), total })
  }
}

impl QobuzClient {
  pub async fn library_revisions(&self) -> Result<LibraryRevisions> {
    self.forbidden_guard()?;
    let response = self
      .http()?
      .get(endpoints::build_url("/user/lastUpdate"))
      .headers(self.authenticated_headers().await?)
      .send()
      .await?;
    self.note_forbidden_status(response.status());
    parse_revisions(read_response(response).await?)
  }

  pub async fn library_favorites_page(
    &self,
    kind: &str,
    limit: u32,
    offset: u64,
  ) -> Result<LibraryPage> {
    page_size(limit)?;
    if !matches!(kind, "albums" | "tracks" | "artists" | "labels" | "awards") {
      return Err(invalid("unknown favorite collection"));
    }
    self.forbidden_guard()?;
    let timestamp = get_timestamp();
    let signature = sign_get_favorites(timestamp, &self.secret().await?);
    let response = self
      .http()?
      .get(endpoints::build_url(paths::FAVORITE_GET_USER_FAVORITES))
      .headers(self.authenticated_headers().await?)
      .query(&[
        ("type", kind.to_owned()),
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
        ("request_ts", timestamp.to_string()),
        ("request_sig", signature),
      ])
      .send()
      .await?;
    self.note_forbidden_status(response.status());
    LibraryPage::parse(read_response(response).await?, kind, limit, offset)
  }

  pub async fn library_purchases_page(
    &self,
    kind: &str,
    limit: u32,
    offset: u64,
  ) -> Result<LibraryPage> {
    page_size(limit)?;
    if !matches!(kind, "albums" | "tracks") {
      return Err(invalid("unknown purchase collection"));
    }
    self.forbidden_guard()?;
    let response = self
      .http()?
      .get(endpoints::build_url(paths::PURCHASE_GET_USER_PURCHASES))
      .headers(self.authenticated_headers().await?)
      .query(&[
        ("type", kind.to_owned()),
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
      ])
      .send()
      .await?;
    self.note_forbidden_status(response.status());
    LibraryPage::parse(read_response(response).await?, kind, limit, offset)
  }

  pub async fn library_playlists_page(&self, limit: u32, offset: u64) -> Result<LibraryPage> {
    page_size(limit)?;
    self.forbidden_guard()?;
    let response = self
      .signed_get_auth(
        &endpoints::build_url(paths::PLAYLIST_GET_USER_PLAYLISTS),
        "playlistgetUserPlaylists",
        &[("limit", limit.to_string()), ("offset", offset.to_string())],
      )
      .await?;
    self.note_forbidden_status(response.status());
    LibraryPage::parse(read_response(response).await?, "playlists", limit, offset)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn revisions_fixture() -> Value {
    json!({"last_update":{"favorite_album":1,"favorite_track":2,"favorite_artist":3,
      "favorite_label":4,"favorite_award":5,"purchase":6,"playlist":7}})
  }

  #[test]
  fn revision_keys_match_each_account_collection() {
    let revisions = parse_revisions(revisions_fixture()).unwrap();
    assert_eq!(revisions.len(), 8);
    for (key, millis) in [
      ("favorite-albums", 1000),
      ("favorite-tracks", 2000),
      ("followed-artists", 3000),
      ("followed-labels", 4000),
      ("followed-awards", 5000),
      ("purchased-albums", 6000),
      ("purchased-tracks", 6000),
      ("playlists", 7000),
    ] {
      assert_eq!(revisions[key], Some(millis));
    }
  }

  #[test]
  fn revision_dates_preserve_milliseconds_and_explicit_unknowns() {
    let mut fixture = revisions_fixture();
    fixture["last_update"]["favorite_album"] = json!(1.125);
    fixture["last_update"]["favorite_track"] = json!("1970-01-01T00:00:02.250Z");
    fixture["last_update"]["favorite_award"] = Value::Null;
    let revisions = parse_revisions(fixture).unwrap();
    assert_eq!(revisions["favorite-albums"], Some(1125));
    assert_eq!(revisions["favorite-tracks"], Some(2250));
    assert_eq!(revisions["followed-awards"], None);
  }

  #[test]
  fn invalid_revision_payloads_cannot_mean_unchanged() {
    let mut missing = revisions_fixture();
    missing["last_update"].as_object_mut().unwrap().remove("favorite_artist");
    assert!(parse_revisions(missing).is_err());
    let mut failure = revisions_fixture();
    failure["status"] = json!("error");
    assert!(parse_revisions(failure).is_err());
    assert!(parse_revisions(json!({"last_update":[]})).is_err());
    for bad in [json!(-1), json!(true), json!("yesterday"), json!([])] {
      let mut fixture = revisions_fixture();
      fixture["last_update"]["playlist"] = bad;
      assert!(parse_revisions(fixture).is_err());
    }
  }

  #[test]
  fn total_keeps_short_server_pages_advancing() {
    let page = LibraryPage::parse(
      json!({"awards":{"items":[{"id":88}],"offset":0,"limit":1,"total":3}}),
      "awards",
      100,
      0,
    )
    .unwrap();
    assert_eq!(page.next_offset, Some(1));
    assert_eq!(page.total, Some(3));
    let last = LibraryPage::parse(
      json!({"awards":{"items":[{"id":90}],"offset":2,"total":3}}),
      "awards",
      100,
      2,
    )
    .unwrap();
    assert_eq!(last.next_offset, None);
  }

  #[test]
  fn empty_collections_are_distinct_from_errors() {
    let empty =
      LibraryPage::parse(json!({"albums":{"items":[],"total":0}}), "albums", 100, 0).unwrap();
    assert!(empty.items.is_empty());
    assert_eq!(empty.next_offset, None);
    for body in [
      json!({"status":"error","albums":{"items":[],"total":0}}),
      json!({"albums":{}}),
      json!({"albums":{"items":[{"id":"a"}],"offset":0}}),
      json!({"albums":{"items":[{"id":""}]}}),
    ] {
      assert!(LibraryPage::parse(body, "albums", 100, 1).is_err());
    }
  }

  #[test]
  fn a_repeated_item_in_a_page_is_kept_once_and_the_offset_counts_every_row() {
    let page = LibraryPage::parse(
      json!({"albums":{"items":[{"id":"a"},{"id":"b"},{"id":"a"}],"offset":4,"total":20}}),
      "albums",
      100,
      4,
    )
    .unwrap();
    let ids: Vec<_> = page.items.iter().map(|item| item["id"].clone()).collect();
    assert_eq!(ids, [json!("a"), json!("b")]);
    assert_eq!(page.next_offset, Some(7), "three rows were sent");
  }

  #[test]
  fn any_end_signal_ends_the_collection_even_against_the_total() {
    for (body, offset) in [
      // An empty page short of the total: the total counted rows never sent.
      (json!({"albums":{"items":[],"total":9}}), 4),
      // More rows than the total.
      (json!({"albums":{"items":[{"id":"a"},{"id":"b"}],"total":1}}), 0),
      // The flag says the end before the total does.
      (json!({"albums":{"items":[{"id":"a"}],"total":9,"has_more":false}}), 0),
      (json!({"albums":{"items":[],"has_more":true}}), 2),
    ] {
      let page = LibraryPage::parse(body.clone(), "albums", 100, offset).unwrap();
      assert_eq!(page.next_offset, None, "{body}");
    }
  }

  #[test]
  fn unknown_totals_continue_until_an_explicit_end_or_an_empty_page() {
    let page =
      LibraryPage::parse(json!({"tracks":{"items":[{"id":4}]}}), "tracks", 100, 0).unwrap();
    assert_eq!(page.next_offset, Some(1));
    let last =
      LibraryPage::parse(json!({"tracks":{"items":[{"id":5}],"has_more":false}}), "tracks", 100, 1)
        .unwrap();
    assert_eq!(last.next_offset, None);
    assert!(
      LibraryPage::parse(json!({"tracks":{"items":[{"id":1},{"id":2}]}}), "tracks", 1, 0).is_err()
    );
  }

  #[test]
  fn raw_membership_dates_survive_paging() {
    for (kind, field) in
      [("albums", "purchased_at"), ("tracks", "favorited_at"), ("playlists", "subscribed_at")]
    {
      let mut item = json!({"id":7});
      item[field] = json!(1790000000);
      let mut body = json!({});
      body[kind] = json!({"items":[item],"total":1});
      assert_eq!(LibraryPage::parse(body, kind, 100, 0).unwrap().items[0][field], 1790000000);
    }
  }

  #[test]
  fn status_and_streamed_size_limits_fail_closed() {
    assert!(page_size(0).is_err());
    assert!(page_size(MAX_PAGE_SIZE).is_ok());
    assert!(page_size(MAX_PAGE_SIZE + 1).is_err());
    assert!(matches!(
      status_result(StatusCode::UNAUTHORIZED, None),
      Err(ApiError::AuthenticationError(_))
    ));
    assert!(matches!(
      status_result(StatusCode::TOO_MANY_REQUESTS, Some("17")),
      Err(ApiError::RateLimited(17))
    ));
    assert!(matches!(
      status_result(StatusCode::SERVICE_UNAVAILABLE, None),
      Err(ApiError::ServerError(503))
    ));
    let mut buffer = vec![0; MAX_RESPONSE_BYTES - 1];
    append_chunk(&mut buffer, &[1]).unwrap();
    let error = append_chunk(&mut buffer, &[2]).unwrap_err();
    assert!(response_too_large(&error));
    assert!(!response_too_large(&ApiError::AuthenticationError("fixture".into())));
    assert_eq!(buffer.len(), MAX_RESPONSE_BYTES);
  }
}
