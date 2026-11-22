use crate::runtime::{RpcFault, RpcResult};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::OnceLock};

pub(super) const MAX_DOCUMENT: usize = 1024 * 1024;
// Leave room for the bounded status message on a cached-page fallback.
const MAX_TEXT: usize = 512 * 1024 - 2048;
const MAX_BLOCKS: usize = 256;
const MAX_IMAGES: usize = 8;
#[path = "embeds.rs"]
mod embeds;
#[path = "images.rs"]
mod images;
use images::{public_image, sized_image};

pub(super) fn plain(input: &str) -> String {
  static RULES: OnceLock<[Regex; 3]> = OnceLock::new();
  let rules = RULES.get_or_init(|| {
    [
      Regex::new(
        r"(?is)<!--.*?-->|<(?:script|style|iframe)\b[^>]*>.*?(?:</(?:script|style|iframe)\s*>|$)",
      )
      .unwrap(),
      Regex::new(r"(?i)<br\b[^>]*>|</(?:p|div|li)\s*>").unwrap(),
      Regex::new(r"<[^>]*>").unwrap(),
    ]
  });
  let cleaned = rules[0].replace_all(input, "");
  let spaced = rules[1].replace_all(&cleaned, "\n");
  let stripped = rules[2].replace_all(&spaced, "");
  html_escape::decode_html_entities(&stripped)
    .chars()
    .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
    .collect::<String>()
    .lines()
    .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

fn text(value: &Value, path: &str) -> String {
  plain(value.pointer(path).and_then(Value::as_str).unwrap_or_default())
}

fn authors(value: &Value) -> Vec<String> {
  value
    .pointer("/credits/by")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .take(16)
    .map(|author| {
      let name = text(author, "/additional_properties/original/byline");
      if name.is_empty() {
        text(author, "/name")
      } else {
        name
      }
    })
    .filter(|name| !name.is_empty() && name.len() <= 512)
    .collect()
}

fn date(value: &Value) -> Option<String> {
  let date = value["display_date"].as_str()?;
  chrono::DateTime::parse_from_rfc3339(date).ok().map(|date| date.to_rfc3339())
}

pub(super) fn teaser(value: &Value) -> Option<Value> {
  let id = value["_id"].as_str().filter(|id| qobuz_api::editorial::valid_story(id))?;
  let title = text(value, "/headlines/basic");
  if title.is_empty() || title.len() > 2048 {
    return None;
  }
  let authors = authors(value);
  Some(json!({"kind":"article","ref":format!("magazine:story:{id}"),"title":title,
    "subtitle":authors.join(" · "),"image":sized_image(&value["promo_items"]["basic"],600),
    "published_at":date(value),"art_fit":"contain"}))
}

pub(super) fn home(value: &Value) -> Result<Vec<Value>, RpcFault> {
  let groups = value.as_array().ok_or_else(|| RpcFault::new(-32000, "Invalid magazine home"))?;
  if groups.iter().any(|group| !group["stories"].is_array()) {
    return Err(RpcFault::new(-32000, "Invalid magazine stories"));
  }
  let mut seen = BTreeSet::new();
  Ok(
    groups
      .iter()
      .flat_map(|group| group["stories"].as_array().into_iter().flatten())
      .filter_map(teaser)
      .filter(|card| seen.insert(card["ref"].as_str().unwrap().to_owned()))
      .take(100)
      .collect(),
  )
}

pub(super) fn sections(value: &Value, language: &str) -> Result<Vec<Value>, RpcFault> {
  fn append(rows: &[Value], language: &str, output: &mut Vec<Value>, depth: usize) {
    if depth > 2 {
      return;
    }
    for row in rows {
      if output.len() == 100 {
        break;
      }
      if let Some(id) = row["_id"].as_str().filter(|id| qobuz_api::editorial::valid_slug(id)) {
        let name = text(row, &format!("/name_tr/name_{language}"));
        let name = if name.is_empty() { text(row, "/name_tr/name_en") } else { name };
        if !name.is_empty()
          && name.len() <= 256
          && !output.iter().any(|option| option["value"] == id)
        {
          output.push(json!({"value":id,"label":name}));
        }
      }
      if let Some(children) = row["children"].as_array() {
        append(children, language, output, depth + 1);
      }
    }
  }
  let mut output = vec![];
  let rows =
    value.as_array().ok_or_else(|| RpcFault::new(-32000, "Invalid magazine navigation"))?;
  append(rows, language, &mut output, 0);
  if !rows.is_empty() && output.is_empty() {
    return Err(RpcFault::new(-32000, "Invalid magazine sections"));
  }
  Ok(output)
}

struct Body {
  blocks: Vec<Value>,
  images: BTreeSet<String>,
  bytes: usize,
  list_items: usize,
}

impl Body {
  fn push(&mut self, block: Value) -> Result<(), RpcFault> {
    self.list_items += block["items"].as_array().map_or(0, Vec::len);
    if self.list_items > 256 {
      return Err(RpcFault::new(-32000, "The article exceeds 256 list items"));
    }
    if ["text", "caption", "alt"]
      .iter()
      .any(|key| block[*key].as_str().is_some_and(|text| text.len() > 64 * 1024))
      || block["items"].as_array().is_some_and(|items| {
        items.len() > 256
          || items.iter().filter_map(Value::as_str).any(|text| text.len() > 64 * 1024)
      })
    {
      return Err(RpcFault::new(-32000, "An article block exceeds its size limit"));
    }
    self.bytes += block.get("text").and_then(Value::as_str).map_or(0, str::len);
    self.bytes += block.get("caption").and_then(Value::as_str).map_or(0, str::len);
    self.bytes += block.get("alt").and_then(Value::as_str).map_or(0, str::len);
    self.bytes += block["items"]
      .as_array()
      .into_iter()
      .flatten()
      .filter_map(Value::as_str)
      .map(str::len)
      .sum::<usize>();
    if self.bytes > MAX_TEXT || self.blocks.len() >= MAX_BLOCKS {
      return Err(RpcFault::new(-32000, "The article exceeds its text or block limit"));
    }
    self.blocks.push(block);
    Ok(())
  }

  fn image(&mut self, value: &Value) -> Result<(), RpcFault> {
    if self.images.len() >= MAX_IMAGES {
      return Ok(());
    }
    if let Some(image) = public_image(value).filter(|url| !self.images.contains(url)) {
      self.images.insert(image.clone());
      let alt = text(value, "/subtitle");
      let alt = if alt.len() <= 4096 { alt } else { String::new() };
      let mut block =
        json!({"type":"image","url":image,"caption":text(value,"/caption"),"alt":alt});
      for key in ["width", "height"] {
        if let Some(size) = value[key].as_u64().filter(|size| (1..=50000).contains(size)) {
          block[key] = size.into();
        }
      }
      self.push(block)?;
    }
    Ok(())
  }

  fn visit(&mut self, elements: &[Value], depth: usize) -> Result<(), RpcFault> {
    if depth > 4 {
      return Ok(());
    }
    for element in elements {
      match element["type"].as_str().unwrap_or_default() {
        "text" | "header" => {
          let content = text(element, "/content");
          if content.is_empty() {
            continue;
          }
          if content.len() > 64 * 1024 {
            return Err(RpcFault::new(-32000, "An article paragraph is too long"));
          }
          let block = if element["type"] == "header" {
            json!({"type":"heading","text":content,"level":element["level"].as_u64().unwrap_or(2).clamp(1,6)})
          } else {
            json!({"type":"paragraph","text":content})
          };
          self.push(block)?;
        }
        "image" => self.image(element)?,
        "gallery" => {
          if let Some(items) = element["content_elements"].as_array() {
            for item in items {
              self.image(item)?;
            }
          }
        }
        "quote" => {
          let mut quoted = Vec::new();
          if let Some(items) = element["content_elements"].as_array() {
            for item in items {
              let content = text(item, "/content");
              if !content.is_empty() {
                quoted.push(content);
              }
            }
          }
          if quoted.is_empty() {
            let content = text(element, "/content");
            if !content.is_empty() {
              quoted.push(content);
            }
          }
          if !quoted.is_empty() {
            self.push(json!({"type":"quote","text":quoted.join("\n")}))?;
          }
        }
        "list" => {
          let items: Vec<_> = element["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| item.as_str().map(plain).unwrap_or_else(|| text(item, "/content")))
            .filter(|item| !item.is_empty())
            .collect();
          if !items.is_empty() {
            self.push(
              json!({"type":"list","items":items,"ordered":element["list_type"] == "ordered"}),
            )?;
          }
        }
        "table" => {
          for row in element["rows"].as_array().into_iter().flatten() {
            let line = row
              .as_array()
              .into_iter()
              .flatten()
              .map(|cell| cell.as_str().map(plain).unwrap_or_else(|| text(cell, "/content")))
              .filter(|cell| !cell.is_empty())
              .collect::<Vec<_>>()
              .join(" | ");
            if !line.is_empty() {
              self.push(json!({"type":"paragraph","text":line}))?;
            }
          }
        }
        "interstitial_link" => {}
        "custom_embed" => self.embed(element)?,
        "raw_html" | "oembed_response" | "video" => {}
        _ => {
          if let Some(items) = element["content_elements"].as_array() {
            self.visit(items, depth + 1)?;
          }
        }
      }
    }
    Ok(())
  }
}

pub(super) fn article(value: &Value, requested: &str) -> RpcResult {
  if value["_id"].as_str() != Some(requested) {
    return Err(RpcFault::new(-32000, "The article identity does not match"));
  }
  let card = teaser(value).ok_or_else(|| RpcFault::new(-32000, "The article has no title"))?;
  let image = public_image(&value["promo_items"]["basic"]);
  let summary = text(value, "/subheadlines/basic");
  let authors = authors(value);
  let published = date(value);
  let mut body = Body {
    blocks: vec![],
    images: image.iter().cloned().collect(),
    list_items: 0,
    bytes: summary.len()
      + authors.iter().map(String::len).sum::<usize>()
      + card["title"].as_str().map_or(0, str::len)
      + requested.len()
      + published.as_ref().map_or(0, String::len),
  };
  let elements = value["content_elements"]
    .as_array()
    .ok_or_else(|| RpcFault::new(-32000, "The article has no readable content"))?;
  body.visit(elements, 0)?;
  if body.blocks.is_empty() {
    return Err(RpcFault::new(-32000, "The article has no readable content"));
  }
  let doc = json!({"layout":"article","title":card["title"],"article":{
    "id":requested,"authors":authors,"published_at":published,"summary":summary,"image":image,"blocks":body.blocks}});
  if body.bytes > MAX_TEXT
    || serde_json::to_vec(&doc).map_or(true, |bytes| bytes.len() > MAX_DOCUMENT - 8192)
  {
    return Err(RpcFault::new(-32000, "The article exceeds its size limit"));
  }
  Ok(doc)
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
