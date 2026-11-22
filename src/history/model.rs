use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

pub(super) const CAPACITY: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Ticket {
  #[serde(rename = "user")]
  pub account: u64,
  #[serde(rename = "track_id")]
  pub track: u64,
  #[serde(rename = "format_id")]
  pub format: u32,
  #[serde(rename = "blob")]
  pub receipt: Option<String>,
  #[serde(rename = "duration")]
  pub seconds: Option<f64>,
  pub preview: bool,
  #[serde(default)]
  pub resource: Option<String>,
  #[serde(default)]
  pub context: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Listening {
  #[serde(rename = "source")]
  pub ticket: Ticket,
  #[serde(rename = "user")]
  pub account: u64,
  #[serde(rename = "started_at")]
  pub began: String,
  pub sequence: u64,
  #[serde(rename = "played_ms")]
  pub rendered: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Envelope {
  #[serde(rename = "id")]
  pub key: String,
  #[serde(rename = "user")]
  pub account: u64,
  #[serde(rename = "start")]
  pub opening: bool,
  #[serde(rename = "event")]
  pub body: Value,
  pub attempts: u32,
  #[serde(rename = "retry_at")]
  pub not_before: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Ledger {
  #[serde(rename = "active")]
  pub playing: HashMap<String, Listening>,
  #[serde(default)]
  pub history: Vec<Value>,
  #[serde(rename = "completed")]
  pub finished: VecDeque<String>,
  #[serde(rename = "queue")]
  pub outbox: VecDeque<Envelope>,
  #[serde(default, rename = "resolved")]
  pub tickets: HashMap<String, Ticket>,
  #[serde(skip)]
  pub last_error: Option<String>,
  #[serde(skip)]
  pub unreadable: bool,
}

impl Ledger {
  pub fn queue(&mut self, key: String, account: u64, opening: bool, body: Value) {
    self.outbox.push_back(Envelope { key, account, opening, body, attempts: 0, not_before: 0 });
  }
  pub fn remember(&mut self, key: &str) {
    self.finished.push_back(key.to_owned());
    while self.finished.len() > CAPACITY {
      self.finished.pop_front();
    }
  }
}
