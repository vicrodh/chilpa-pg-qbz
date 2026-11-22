use crate::catalog::{parse_reference, CatalogService, RpcFault, RpcResult};
use qobuz_api::{QobuzLyricsContent, QobuzLyricsDocument, QobuzLyricsUrls};
use serde_json::{json, Map, Value};
use std::time::Duration;

const CDN_SUFFIX: &str = ".cloudfront.net";
const MAX_BYTES: usize = 1024 * 1024;
const PATIENCE: Duration = Duration::from_secs(8);

impl CatalogService {
  pub(crate) async fn load_lyrics(&self, request: Value) -> RpcResult {
    let id: u64 =
      parse_reference(&request, "track")?.parse().map_err(|_| RpcFault::invalid("track"))?;
    let language = language_of(&request);
    self.require_session().await?;
    let lookup = async {
      tokio::join!(
        self.fetch_lyrics(id, None),
        async {
          match language.as_deref() {
            Some(language) => self.fetch_lyrics(id, Some(language)).await.ok().flatten(),
            None => None,
          }
        },
        self.lookup_track(id),
      )
    };
    let (original, translated, track) = tokio::time::timeout(PATIENCE, lookup)
      .await
      .map_err(|_| RpcFault::new(-32000, "the lyrics took too long"))?;
    let Some(document) = original? else {
      return Ok(json!({"status": "none"}));
    };
    let translation = translated
      .as_ref()
      .and_then(|other| other.translation.as_ref())
      .or(document.translation.as_ref());
    let track = track.ok();
    Ok(answer(&document, translation, language.as_deref(), track.as_ref()))
  }

  async fn fetch_lyrics(
    &self,
    id: u64,
    language: Option<&str>,
  ) -> Result<Option<QobuzLyricsDocument>, RpcFault> {
    let (status, retry_after, body) = self.session.api.get_lyrics_url_reply(id, language).await?;
    let Some(address) = address(status, retry_after, &body)? else {
      return Ok(None);
    };
    if !from_the_cdn(&address) {
      return Err(RpcFault::new(
        -32000,
        "the lyrics are at an address that is not the service's CDN",
      ));
    }
    let response = self
      .session
      .api
      .get_http()
      .get(&address)
      .send()
      .await
      .map_err(|e| RpcFault::new(-32000, e.without_url().to_string()))?;
    let status = response.status().as_u16();
    let retry_after = response
      .headers()
      .get("retry-after")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.trim().parse::<u64>().ok());
    if response.content_length().is_some_and(|length| length > MAX_BYTES as u64) {
      return Err(RpcFault::new(-32000, "the lyrics document is too large"));
    }
    let body =
      response.text().await.map_err(|e| RpcFault::new(-32000, e.without_url().to_string()))?;
    if body.len() > MAX_BYTES {
      return Err(RpcFault::new(-32000, "the lyrics document is too large"));
    }
    document(status, retry_after, &body)
  }
}

fn language_of(request: &Value) -> Option<String> {
  request
    .get("language")
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|l| l.len() == 2 && l.chars().all(|c| c.is_ascii_alphabetic()))
    .map(str::to_lowercase)
}

fn address(status: u16, retry_after: Option<u64>, body: &str) -> Result<Option<String>, RpcFault> {
  match status {
    200 => {
      let urls: QobuzLyricsUrls = serde_json::from_str(body).map_err(|e| {
        RpcFault::new(-32000, format!("the answer about lyrics cannot be read: {e}"))
      })?;
      Ok(urls.lyrics_url.filter(|url| !url.trim().is_empty()))
    }
    404 => Ok(None),
    429 => Err(RpcFault::busy("the service asked for a pause", retry_after)),
    other => Err(RpcFault::new(-32000, format!("the service answered {other} about lyrics"))),
  }
}

fn document(
  status: u16,
  retry_after: Option<u64>,
  body: &str,
) -> Result<Option<QobuzLyricsDocument>, RpcFault> {
  match status {
    200..=299 => serde_json::from_str::<QobuzLyricsDocument>(body)
      .map(Some)
      .map_err(|e| RpcFault::new(-32000, format!("the lyrics document cannot be read: {e}"))),
    429 => Err(RpcFault::busy("the lyrics CDN asked for a pause", retry_after)),
    500..=599 => Err(RpcFault::new(-32000, format!("the lyrics CDN answered {status}"))),
    _ => Ok(None),
  }
}

fn from_the_cdn(address: &str) -> bool {
  url::Url::parse(address).ok().is_some_and(|url| {
    url.scheme() == "https" && url.host_str().is_some_and(|host| host.ends_with(CDN_SUFFIX))
  })
}

fn answer(
  document: &QobuzLyricsDocument,
  translation: Option<&QobuzLyricsContent>,
  asked: Option<&str>,
  track: Option<&Value>,
) -> Value {
  let Some(original) = document.original.as_ref() else {
    return json!({"status": "none"});
  };
  let lines = lines_of(original);
  if lines.is_empty() {
    return json!({"status": "none"});
  }
  let timing = if lines.iter().any(|l| !l.segments.is_empty()) {
    "word"
  } else if lines.iter().any(|l| l.start.is_some()) {
    "line"
  } else {
    "unsynced"
  };
  let mut out = json!({
      "schema": 1,
      "timing": timing,
      "lines": lines.iter().map(Entry::to_json).collect::<Vec<_>>(),
  });
  if let Some(language) = original.lang().map(str::trim).filter(|l| !l.is_empty()) {
    out["language"] = json!(language);
  }
  let credits: Vec<Value> = document
    .writers
    .as_deref()
    .unwrap_or("")
    .split(',')
    .map(str::trim)
    .filter(|name| !name.is_empty())
    .map(|name| json!({"name": name, "role": "songwriter"}))
    .collect();
  if !credits.is_empty() {
    out["credits"] = Value::Array(credits);
  }
  if let Some(layer) = translation.and_then(|t| layer(&lines, original, t, asked)) {
    out["layers"] = json!([layer]);
  }
  let mut found = json!({"status": "found", "lyrics": {"format": "document", "document": out}});
  let mut fingerprint = Map::new();
  for key in ["duration_ms", "isrc"] {
    if let Some(value) = track.and_then(|t| t.get(key)).filter(|v| !v.is_null()) {
      fingerprint.insert(key.into(), value.clone());
    }
  }
  if !fingerprint.is_empty() {
    found["fingerprint"] = Value::Object(fingerprint);
  }
  found
}

struct Entry {
  start: Option<u64>,
  end: Option<u64>,
  text: String,
  segments: Vec<Value>,
}

impl Entry {
  fn to_json(&self) -> Value {
    let mut line = json!({"text": self.text});
    if let Some(start) = self.start {
      line["start_ms"] = json!(start);
    }
    if let Some(end) = self.end {
      line["end_ms"] = json!(end);
    }
    if !self.segments.is_empty() {
      line["segments"] = Value::Array(self.segments.clone());
    }
    line
  }
}

fn ms(value: i64) -> Option<u64> {
  (value >= 0).then_some((value as u64).min(u64::from(u32::MAX)))
}

fn lines_of(content: &QobuzLyricsContent) -> Vec<Entry> {
  match content {
    QobuzLyricsContent::Plain { lines, .. } => lines
      .iter()
      .map(|l| l.line.trim())
      .filter(|text| !text.is_empty())
      .map(|text| Entry { start: None, end: None, text: text.to_string(), segments: Vec::new() })
      .collect(),
    QobuzLyricsContent::Synced { lines, .. } => {
      let mut out: Vec<Entry> = Vec::new();
      for line in lines {
        let words: Vec<_> = line.words.iter().filter(|w| !w.word.trim().is_empty()).collect();
        let timed: Vec<_> =
          words.iter().filter_map(|w| ms(w.start).map(|start| (w, start))).collect();
        let text = match line.line.trim() {
          "" => words.iter().map(|w| w.word.trim()).collect::<Vec<_>>().join(" "),
          text => text.to_string(),
        };
        if text.is_empty() {
          continue;
        }
        let start = line.start.and_then(ms).or_else(|| timed.first().map(|(_, start)| *start));
        let end = line.end.and_then(ms).zip(start).map(|(end, start)| end.max(start));
        let last = timed.len().saturating_sub(1);
        let segments = timed
          .iter()
          .enumerate()
          .map(|(at, (word, start))| {
            let mut segment =
              json!({"text": word.word.trim(), "start_ms": start, "space_after": at < last});
            if let Some(end) = ms(word.end).filter(|end| end >= start) {
              segment["end_ms"] = json!(end);
            }
            segment
          })
          .collect();
        out.push(Entry { start, end, text, segments });
      }
      if out.iter().any(|l| l.start.is_some()) {
        out.retain(|l| l.start.is_some());
        out.sort_by_key(|l| l.start);
      }
      out
    }
  }
}

fn layer(
  lines: &[Entry],
  original: &QobuzLyricsContent,
  translation: &QobuzLyricsContent,
  asked: Option<&str>,
) -> Option<Value> {
  let language = translation.lang().map(str::trim).filter(|l| !l.is_empty()).or(asked)?;
  if original.lang().is_some_and(|own| own.trim().eq_ignore_ascii_case(language)) {
    return None;
  }
  let translated = lines_of(translation);
  let timed =
    lines.iter().all(|l| l.start.is_some()) && translated.iter().all(|l| l.start.is_some());
  let mut pairs: Vec<Value> = Vec::new();
  if timed {
    let mut from = 0;
    for line in &translated {
      if let Some(at) = lines[from..].iter().position(|l| l.start == line.start) {
        pairs.push(json!({"line": from + at, "text": line.text}));
        from += at + 1;
      }
    }
  } else if lines.len() == translated.len() && lines.iter().all(|l| l.start.is_none()) {
    pairs = translated
      .iter()
      .enumerate()
      .map(|(at, line)| json!({"line": at, "text": line.text}))
      .collect();
  }
  (!pairs.is_empty()).then(|| json!({"kind": "translation", "language": language, "lines": pairs}))
}

#[cfg(test)]
#[path = "checks/subtitles_tests.rs"]
mod checks;
