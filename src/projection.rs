//! Projection rules shared by catalog responses and native pages.
use serde_json::{json, Map, Value};

#[derive(Clone, Copy)]
struct Document<'a>(&'a Value);

impl<'a> Document<'a> {
  fn field(self, path: &str) -> Option<&'a Value> {
    self.0.pointer(&format!("/{path}")).filter(|value| !value.is_null())
  }
  fn string(self, paths: &[&str]) -> Option<String> {
    paths.iter().filter_map(|path| self.field(path)).find_map(|value| match value {
      Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
      Value::Number(number) => Some(number.to_string()),
      _ => None,
    })
  }
  fn numeric(self, paths: &[&str]) -> Option<f64> {
    paths.iter().find_map(|path| self.field(path)?.as_f64())
  }
  fn reference(self, kind: &str, paths: &[&str]) -> Option<String> {
    self.string(paths).map(|identifier| format!("{kind}:{identifier}"))
  }
  fn title(self) -> Option<String> {
    let mut title = self.string(&["title"])?;
    if let Some(version) = self.string(&["version"]).filter(|version| !title.contains(version)) {
      title.push_str(&format!(" ({version})"));
    }
    Some(title)
  }
  fn release_date(self) -> Option<String> {
    self.string(&["release_date_original", "dates/original", "release_date_stream", "dates/stream"])
  }
  fn year(self) -> Option<i64> {
    self.release_date()?.get(..4)?.parse().ok()
  }
  fn category(self) -> Option<&'static str> {
    ["release_type", "product_type"].into_iter().filter_map(|key| self.string(&[key])).find_map(
      |text| match text.to_ascii_lowercase().as_str() {
        "album" => Some("album"),
        "epmini" | "ep" => Some("ep"),
        "single" => Some("single"),
        "compilation" => Some("compilation"),
        "live" => Some("live"),
        _ => None,
      },
    )
  }
  fn quality(self) -> Option<Value> {
    let depth = self.numeric(&["maximum_bit_depth", "audio_info/maximum_bit_depth"])?;
    let rate = self.numeric(&["maximum_sampling_rate", "audio_info/maximum_sampling_rate"])?;
    Some(json!({"codec":"flac","bits":depth as u32,"rate":(rate * 1000.0).round() as u32}))
  }
  fn artwork(self) -> Option<String> {
    if let (Some(hash), Some(extension)) =
      (self.string(&["images/portrait/hash"]), self.string(&["images/portrait/format"]))
    {
      return Some(format!(
        "https://static.qobuz.com/images/artists/covers/large/{hash}.{extension}"
      ));
    }
    let address = self.string(&[
      "image_rectangle/0",
      "image_rectangle_mini/0",
      "image/large",
      "image/mega",
      "image/extralarge",
      "image/medium",
      "image/thumbnail",
      "image/small",
      "image/rectangle",
      "images/portrait/url",
      "images/portrait",
      "images/large",
      "images/medium",
      "images/small",
      "images/rectangle/0",
      "images300/0",
      "image/covers/0",
      "images/0",
      "images150/0",
      "picture",
      "image",
    ])?;
    artwork_url(address)
  }
}

fn artwork_url(address: String) -> Option<String> {
  let address = if address.starts_with("//") { format!("https:{address}") } else { address };
  let parsed = url::Url::parse(&address).ok()?;
  if !matches!(parsed.scheme(), "http" | "https") {
    return None;
  }
  if parsed.host_str() == Some("static.qobuz.com") {
    if let Some(cover) = qobuz_models::qobuz_cover_at_px(&address, 600) {
      return Some(cover);
    }
  }
  Some(address)
}

struct Card(Map<String, Value>);
impl Card {
  fn new(kind: &str, source: Document<'_>, dates: bool) -> Self {
    let mut card = Self(Map::new());
    card.set("kind", Some(kind));
    if dates {
      card.set(
        "added_at",
        source.numeric(&["favorited_at", "purchased_at", "created_at"]).map(|n| n as i64),
      );
      card.set("updated_at", source.numeric(&["updated_at"]).map(|n| n as i64));
      card.set("release_date", source.release_date());
    }
    card
  }
  fn set<T: Into<Value>>(&mut self, key: &str, value: Option<T>) {
    if let Some(value) = value {
      self.0.insert(key.to_owned(), value.into());
    }
  }
  fn strings(&mut self, source: Document<'_>, mapping: &[(&str, &[&str])]) {
    for (destination, candidates) in mapping {
      self.set(destination, source.string(candidates));
    }
  }
  fn duration(&mut self, source: Document<'_>) {
    self.set("duration_ms", source.numeric(&["duration"]).map(|seconds| (seconds * 1000.0) as u64));
  }
  fn genre(&mut self, source: Document<'_>) {
    if let Some(name) =
      source.string(&["genre/name"]).filter(|name| !name.eq_ignore_ascii_case("Music"))
    {
      self.set("genres", Some(json!([name])));
      self.set("genre", Some(name));
    }
  }
  fn delivery(&mut self, source: Document<'_>) {
    self.set("available", source.0.get("streamable").and_then(Value::as_bool));
    self.set("quality", source.quality());
    self.set("explicit", source.field("parental_warning").and_then(Value::as_bool));
  }
  fn finish(self) -> Value {
    let mut card = Value::Object(self.0);
    image_fallback(&mut card);
    card
  }
}

pub(crate) fn image(source: &Value) -> Option<String> {
  Document(source).artwork()
}

pub(crate) fn image_fallback(card: &mut Value) {
  let glyph = match card["kind"].as_str() {
    Some("artist") => "user-star",
    Some("label") => "disc",
    Some("award") => "award",
    _ => return,
  };
  card["image_fallback"] = json!(glyph);
}

pub fn album_card(raw: &Value) -> Value {
  let source = Document(raw);
  let mut card = Card::new("album", source, true);
  card.set("ref", source.reference("album", &["id"]));
  card.set("title", source.title());
  card.strings(
    source,
    &[
      (
        "artist",
        &["artist/name/display", "artist/name", "artists/0/name/display", "artists/0/name"],
      ),
      ("label", &["label/name"]),
      ("description", &["description"]),
      ("copyright", &["copyright"]),
      ("upc", &["upc"]),
    ],
  );
  card.set("artist_ref", source.reference("artist", &["artist/id", "artists/0/id"]));
  card.set("label_ref", source.reference("label", &["label/id"]));
  card.set("year", source.year());
  card.set("image", source.artwork());
  card.duration(source);
  card.set("tracks", source.numeric(&["tracks_count", "track_count"]).map(|count| count as u64));
  card.genre(source);
  card.set("composers", source.string(&["composer/name"]).map(|name| vec![name]));
  card.set("release_type", source.category());
  let booklets: Vec<_> = collection(raw.get("goodies")).iter().filter_map(|value| {
    let goodie = Document(value);
    let url = goodie.string(&["original_url", "url", "file_url"])?;
    if !url.starts_with("http://") && !url.starts_with("https://") { return None; }
    Some(json!({"url":url,"name":goodie.string(&["name","description"]).unwrap_or_else(|| "Booklet".into())}))
  }).collect();
  if !booklets.is_empty() {
    card.set("booklets", Some(json!(booklets)));
  }
  card.set("awards", source.field("awards").cloned());
  card.delivery(source);
  card.finish()
}

pub(crate) fn normalization(raw: &Value) -> Option<Value> {
  let gain =
    raw.pointer("/audio_info/replaygain_track_gain")?.as_f64().filter(|gain| gain.is_finite())?;
  let peak = raw
    .pointer("/audio_info/replaygain_track_peak")
    .and_then(Value::as_f64)
    .filter(|peak| peak.is_finite() && *peak > 0.0);
  Some(json!({"gain_db":gain,"peak":peak,"source":"provider","scope":"track"}))
}

pub fn track_card(raw: &Value, parent: Option<&Value>) -> Value {
  let source = Document(raw);
  let parent = parent.map(Document);
  let record = source.field("album").map(Document).or(parent);
  let mut card = Card::new("track", source, true);
  card.set("ref", source.reference("track", &["id"]));
  card.set("title", source.title());
  if let Some(artist) = qobuz_models::artist_display::track_artist(raw, parent.map(|album| album.0))
  {
    card.set("artist", Some(artist.name));
    card.set("artist_ref", artist.id.map(|id| format!("artist:{id}")));
  }
  card.set("album", source.string(&["album/title"]).or_else(|| parent?.title()));
  card.set(
    "album_ref",
    source.reference("album", &["album/id"]).or_else(|| parent?.reference("album", &["id"])),
  );
  card.set(
    "image",
    source
      .string(&["album/image/large", "album/image/small"])
      .and_then(artwork_url)
      .or_else(|| parent?.artwork()),
  );
  card.set(
    "year",
    source.field("album").and_then(|album| Document(album).year()).or_else(|| parent?.year()),
  );
  card.duration(source);
  for (target, candidates) in [
    ("track_number", &["track_number", "physical_support/track_number"]),
    ("disc_number", &["media_number", "physical_support/media_number"]),
  ] {
    card.set(target, source.numeric(candidates).map(|number| number as u64));
  }
  let work = source.string(&["work"]);
  if work.is_some() {
    card.set("composer", source.string(&["composer/name"]));
    card.set("composer_ref", source.reference("artist", &["composer/id"]));
  }
  card.set("work", work);
  card.set("composers", source.string(&["composer/name"]).map(|name| vec![name]));
  if let Some(record) = record {
    card.set("label", record.string(&["label/name"]));
    card.set("label_ref", record.reference("label", &["label/id"]));
    card.genre(record);
    card.set("release_type", record.category());
    card.set("release_date", record.release_date());
  }
  card.strings(
    source,
    &[("performers", &["performers"]), ("copyright", &["copyright"]), ("isrc", &["isrc"])],
  );
  card.set("normalization", normalization(raw));
  card.delivery(source);
  card.finish()
}

pub fn artist_card(raw: &Value) -> Value {
  let source = Document(raw);
  let mut card = Card::new("artist", source, true);
  card.set("ref", source.reference("artist", &["id"]));
  card.set("title", source.string(&["name/display", "name"]));
  card.set("image", source.artwork());
  card.set("albums", source.numeric(&["albums_count"]).map(|count| count as u64));
  card.finish()
}

pub fn label_card(raw: &Value) -> Value {
  let source = Document(raw);
  let mut card = Card::new("label", source, false);
  card.set(
    "ref",
    source
      .string(&["id"])
      .and_then(|text| text.parse::<u64>().ok())
      .map(|identifier| format!("label:{identifier}")),
  );
  card.strings(source, &[("title", &["name"]), ("description", &["description"])]);
  card.set("image", source.artwork());
  card.set("art_fit", Some("contain"));
  card.finish()
}

pub fn playlist_card(raw: &Value) -> Value {
  let source = Document(raw);
  let mut card = Card::new("playlist", source, true);
  card.set("ref", source.reference("playlist", &["id"]));
  let image = source.artwork();
  if image.is_some() {
    card.set("art_fit", Some("contain"));
  }
  card.set("image", image);
  card.strings(
    source,
    &[
      ("title", &["name", "title"]),
      ("artist", &["owner/name"]),
      ("description", &["description"]),
    ],
  );
  card.set("tracks", source.numeric(&["tracks_count"]).map(|count| count as u64));
  card.duration(source);
  card.set("tags", source.field("tags").cloned());
  card.finish()
}

pub fn collection(raw: Option<&Value>) -> Vec<Value> {
  raw
    .and_then(|value| value.as_array().or_else(|| value.get("items")?.as_array()))
    .cloned()
    .unwrap_or_default()
}
