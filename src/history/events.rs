use super::model::{Ledger, Listening, CAPACITY};
use serde_json::{json, Value};

enum EventKind {
  Start,
  Progress,
  End,
}
struct Event<'a> {
  key: &'a str,
  kind: EventKind,
  body: &'a Value,
}

fn required<'a>(body: &'a Value, key: &str) -> Result<&'a str, String> {
  body[key]
    .as_str()
    .filter(|text| !text.is_empty() && text.len() <= 128)
    .ok_or_else(|| format!("Invalid {key}"))
}

impl<'a> Event<'a> {
  fn read(body: &'a Value) -> Result<Self, String> {
    let key = required(body, "playback_id")?;
    let kind = match required(body, "kind")? {
      "started" => EventKind::Start,
      "progress" => EventKind::Progress,
      "ended" => EventKind::End,
      _ => return Err("Invalid playback event kind".into()),
    };
    Ok(Self { key, kind, body })
  }
}

impl Ledger {
  pub fn advance(&mut self, account: u64, body: &Value) -> Result<(), String> {
    let event = Event::read(body)?;
    if self.finished.iter().any(|key| key == event.key) {
      return Ok(());
    }
    match event.kind {
      EventKind::Start => self.begin_listening(account, event),
      EventKind::Progress | EventKind::End => self.update_listening(account, event),
    }
  }

  fn begin_listening(&mut self, account: u64, event: Event<'_>) -> Result<(), String> {
    if self.playing.contains_key(event.key) {
      return Ok(());
    }
    if self.playing.len() >= 16 || self.outbox.len() >= CAPACITY {
      return Err("Playback report queue is full".into());
    }
    let ticket = self
      .tickets
      .get(required(event.body, "resolution_id")?)
      .cloned()
      .ok_or("Unknown stream resolution")?;
    if ticket.account != account {
      return Err("Stream resolution belongs to another account".into());
    }
    if ticket.preview {
      self.remember(event.key);
      return Ok(());
    }
    let timestamp = match event.body.get("started_at") {
      None => chrono::Utc::now(),
      Some(value) => value
        .as_i64()
        .filter(|number| *number > 0)
        .and_then(|number| chrono::DateTime::from_timestamp(number, 0))
        .filter(|date| *date <= chrono::Utc::now() + chrono::Duration::seconds(60))
        .ok_or("Invalid playback start timestamp")?,
    };
    if ticket.format != 0 {
      self.queue(format!("{}:start", event.key), account, true,
      json!({"track_id":ticket.track,"date":timestamp.timestamp(),"user_id":account,"format_id":ticket.format}));
    }
    self.history.insert(
      0,
      json!({"user":account,"track_id":ticket.track,"played_at":timestamp.timestamp(),
      "playback_id":event.key,"context":ticket.context}),
    );
    self.history.truncate(1000);
    self.playing.insert(
      event.key.to_owned(),
      Listening {
        ticket,
        account,
        began: timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        sequence: 0,
        rendered: 0,
      },
    );
    Ok(())
  }

  fn update_listening(&mut self, account: u64, event: Event<'_>) -> Result<(), String> {
    let sequence = event.body["sequence"]
      .as_u64()
      .filter(|sequence| *sequence > 0)
      .ok_or("Invalid playback sequence")?;
    let rendered = event.body["played_ms"].as_u64().ok_or("Missing audible duration")?;
    let playback = self.playing.get_mut(event.key).ok_or("Playback has not started")?;
    if playback.account != account {
      return Err("Playback belongs to another account".into());
    }
    if sequence <= playback.sequence {
      return Ok(());
    }
    if rendered < playback.rendered {
      return Err("Audible duration must not decrease".into());
    }
    playback.sequence = sequence;
    playback.rendered = rendered;
    if matches!(event.kind, EventKind::End) {
      self.end_listening(account, event.key)?;
    }
    Ok(())
  }

  fn end_listening(&mut self, account: u64, key: &str) -> Result<(), String> {
    if self.outbox.len() >= CAPACITY {
      return Err("Playback report queue is full".into());
    }
    let playback = self.playing.remove(key).expect("validated playback");
    let measured = playback.rendered / 1000;
    let seconds = playback
      .ticket
      .seconds
      .filter(|duration| duration.is_finite() && *duration >= 0.0)
      .map_or(measured, |limit| measured.min(limit as u64));
    if seconds > 0 {
      match playback.ticket.receipt.filter(|receipt| !receipt.is_empty()) {
        Some(receipt) => self.queue(format!("{key}:end"), account, false,
          json!({"blob":receipt,"track_context_uuid":key,"start_stream":playback.began,"online":true,"local":false,"duration":seconds})),
        None => self.last_error = Some("The stream has no reporting blob; no end event was invented".into()),
      }
    }
    self.remember(key);
    Ok(())
  }
}
