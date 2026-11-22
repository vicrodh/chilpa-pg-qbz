use crate::memo::ReplyMemo;
use serde_json::{json, Value};
use std::time::Duration;

/// Small, account-scoped records derived from already fetched catalog replies.
#[derive(Default)]
pub(super) struct PlaybackMetadata(ReplyMemo);

impl PlaybackMetadata {
  pub fn remember(&self, reply: &Value, lifetime: Duration) {
    let cards = std::iter::once(reply)
      .chain(reply["tracks"].as_array().into_iter().flatten())
      .chain(reply["items"].as_array().into_iter().flatten());
    for card in cards.take(512) {
      if card["kind"] != "track" {
        continue;
      }
      let Some(id) = card["ref"]
        .as_str()
        .and_then(|value| value.split('@').next()?.strip_prefix("track:"))
        .and_then(|value| value.parse::<u64>().ok())
      else {
        continue;
      };
      let Some(duration) = card["duration_ms"].as_u64().filter(|value| *value > 0) else {
        continue;
      };
      let Some(available) = card["available"].as_bool() else { continue };
      let mut metadata =
        json!({"track":id.to_string(),"duration_ms":duration,"available":available});
      if let Some(normalization) = card.get("normalization") {
        metadata["normalization"] = normalization.clone();
      }
      self.0.put(&id.to_string(), lifetime.min(Duration::from_secs(300)), &metadata);
    }
  }

  pub fn get(&self, track: u64) -> Option<Value> {
    self.0.get(&track.to_string())
  }
  pub fn clear(&self) {
    self.0.clear();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn catalog_records_preserve_identity_availability_and_normalization() {
    let cache = PlaybackMetadata::default();
    cache.remember(&json!({"tracks":[
      {"kind":"track","ref":"track:42@playlist:7","duration_ms":180000,"available":false,"normalization":{"gain_db":-2}},
      {"kind":"album","ref":"album:43","duration_ms":180000,"available":true},
      {"kind":"track","ref":"track:44","duration_ms":180000},
      {"kind":"track","ref":"track:45","duration_ms":0,"available":true}
    ]}), Duration::from_secs(60));
    assert_eq!(
      cache.get(42),
      Some(
        json!({"track":"42","duration_ms":180000,"available":false,"normalization":{"gain_db":-2}})
      )
    );
    for id in [43, 44, 45] {
      assert!(cache.get(id).is_none());
    }
    cache.clear();
    assert!(cache.get(42).is_none());
  }

  #[test]
  fn expired_catalog_data_is_not_reused() {
    let cache = PlaybackMetadata::default();
    cache.remember(
      &json!({"kind":"track","ref":"track:42","duration_ms":180000,"available":true}),
      Duration::ZERO,
    );
    assert!(cache.get(42).is_none());
  }
}
