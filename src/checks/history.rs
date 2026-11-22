use super::*;
use model::{Ledger, Ticket};

fn fixture(preview: bool, receipt: Option<&str>) -> Ledger {
  let mut ledger = Ledger::default();
  ledger.tickets.insert(
    "ticket".into(),
    Ticket {
      account: 7,
      track: 42,
      format: 27,
      receipt: receipt.map(str::to_owned),
      seconds: Some(200.0),
      preview,
      resource: None,
      context: Some("playlist:9".into()),
    },
  );
  ledger
}
fn start() -> Value {
  json!({"kind":"started","playback_id":"play","resolution_id":"ticket","started_at":1790000000})
}

#[test]
fn test_event_sequence_table() {
  let mut ledger = fixture(false, Some("fixture-receipt"));
  let cases = [
    (7, start(), true, 1),
    (7, start(), true, 1),
    (
      7,
      json!({"kind":"progress","playback_id":"play","sequence":2,"played_ms":12500,"position_ms":180000}),
      true,
      1,
    ),
    (7, json!({"kind":"progress","playback_id":"play","sequence":1,"played_ms":999000}), true, 1),
    (7, json!({"kind":"progress","playback_id":"play","sequence":3,"played_ms":10}), false, 1),
    (8, json!({"kind":"ended","playback_id":"play","sequence":3,"played_ms":12750}), false, 1),
    (7, json!({"kind":"ended","playback_id":"play","sequence":3,"played_ms":12750}), true, 2),
    (7, json!({"kind":"ended","playback_id":"play"}), true, 2),
  ];
  for (account, event, accepted, pending) in cases {
    assert_eq!(ledger.advance(account, &event).is_ok(), accepted);
    assert_eq!(ledger.outbox.len(), pending);
  }
  assert_eq!(ledger.outbox[1].body["duration"], 12);
  assert_eq!(ledger.outbox[1].body["blob"], "fixture-receipt");
  assert_eq!(ledger.history.len(), 1);
  assert_eq!(ledger.history[0]["context"], "playlist:9");
}

#[test]
fn test_preview_zero_duration_and_missing_receipt() {
  for (preview, duration, receipt, expected, warning) in [
    (true, 5000, Some("fixture"), 0, false),
    (false, 0, Some("fixture"), 1, false),
    (false, 5000, None, 1, true),
    (false, 999000, Some("fixture"), 2, false),
  ] {
    let mut ledger = fixture(preview, receipt);
    ledger.advance(7, &start()).unwrap();
    ledger
      .advance(7, &json!({"kind":"ended","playback_id":"play","sequence":1,"played_ms":duration}))
      .unwrap();
    assert_eq!(ledger.outbox.len(), expected);
    assert_eq!(ledger.last_error.is_some(), warning);
    if expected == 2 {
      assert_eq!(ledger.outbox[1].body["duration"], 200);
    }
    assert_eq!(ledger.history.is_empty(), preview);
  }
}

#[test]
fn test_ticket_account_and_replay_identity() {
  let mut ledger = fixture(false, Some("fixture"));
  assert!(ledger.outbox.is_empty());
  assert!(ledger.advance(8, &start()).is_err());
  ledger.advance(7, &start()).unwrap();
  ledger
    .advance(7, &json!({"kind":"ended","playback_id":"play","sequence":1,"played_ms":4000}))
    .unwrap();
  let mut replay = start();
  replay["playback_id"] = json!("second");
  ledger.advance(7, &replay).unwrap();
  assert_eq!(ledger.outbox.len(), 3);
  assert_eq!(ledger.history.len(), 2);
}

#[tokio::test]
async fn test_durable_transactions_and_old_schema() {
  let directory = tempfile::tempdir().unwrap();
  let journal = PlaybackJournal::new(directory.path());
  *journal.ledger.lock().await = fixture(false, Some("fixture"));
  journal.record(7, &start()).await.unwrap();
  journal
    .record(7, &json!({"kind":"ended","playback_id":"play","sequence":1,"played_ms":3000}))
    .await
    .unwrap();
  let snapshot = std::fs::read(directory.path().join("reports.json")).unwrap();
  let raw: Value = serde_json::from_slice(&snapshot).unwrap();
  assert!(raw["resolved"]["ticket"]["user"].is_u64());
  assert_eq!(raw["queue"][1]["event"]["duration"], 3);
  let restored = PlaybackJournal::new(directory.path());
  restored.recover().await.unwrap();
  assert_eq!(restored.inspect().await["queued"], 2);
  assert_eq!(restored.recent(7).await.len(), 1);
  assert!(restored.recent(8).await.is_empty());
  assert!(restored.record(7, &json!({"kind":"unknown","playback_id":"invalid"})).await.is_err());
  assert_eq!(std::fs::read(directory.path().join("reports.json")).unwrap(), snapshot);
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
      std::fs::metadata(directory.path().join("reports.json")).unwrap().permissions().mode()
        & 0o777,
      0o600
    );
  }
}

#[tokio::test]
async fn test_corrupt_ledger_requires_reset() {
  let directory = tempfile::tempdir().unwrap();
  std::fs::write(directory.path().join("reports.json"), "{broken").unwrap();
  let journal = PlaybackJournal::new(directory.path());
  journal.recover().await.unwrap();
  assert!(journal.record(7, &start()).await.is_err());
  assert_eq!(std::fs::read_to_string(directory.path().join("reports.json")).unwrap(), "{broken");
  journal.reset().await.unwrap();
  assert_eq!(journal.inspect().await["queued"], 0);
}

#[tokio::test]
async fn test_plugin_restart_keeps_accepting_live_playback_events() {
  let directory = tempfile::tempdir().unwrap();
  let before = PlaybackJournal::new(directory.path());
  *before.ledger.lock().await = fixture(false, Some("receipt"));
  before.record(7, &start()).await.unwrap();
  before
    .record(7, &json!({"kind":"progress","playback_id":"play","sequence":1,"played_ms":12500}))
    .await
    .unwrap();
  let resumed = PlaybackJournal::new(directory.path());
  resumed.recover().await.unwrap();
  assert_eq!(resumed.inspect().await["active"], 1);
  assert_eq!(resumed.inspect().await["queued"], 1);
  resumed
    .record(7, &json!({"kind":"progress","playback_id":"play","sequence":2,"played_ms":25000}))
    .await
    .unwrap();
  let end = json!({"kind":"ended","playback_id":"play","sequence":3,"played_ms":25500});
  resumed.record(7, &end).await.unwrap();
  resumed.record(7, &end).await.unwrap();
  let ledger = resumed.ledger.lock().await;
  assert!(ledger.playing.is_empty());
  assert_eq!(ledger.outbox.len(), 2);
  assert_eq!(ledger.outbox[1].body["duration"], 25);
  assert_eq!(ledger.outbox[1].body["blob"], "receipt");
  assert_eq!(ledger.outbox[1].body["track_context_uuid"], "play");
}

#[tokio::test]
async fn test_restored_playback_uses_authoritative_shutdown_checkpoint() {
  let directory = tempfile::tempdir().unwrap();
  let before = PlaybackJournal::new(directory.path());
  *before.ledger.lock().await = fixture(false, Some("receipt"));
  before.record(7, &start()).await.unwrap();
  before
    .record(7, &json!({"kind":"progress","playback_id":"play","sequence":1,"played_ms":12500}))
    .await
    .unwrap();
  let restored = PlaybackJournal::new(directory.path());
  restored.recover().await.unwrap();
  let end =
    json!({"kind":"ended","playback_id":"play","sequence":2,"played_ms":19750,"reason":"shutdown"});
  restored.record(7, &end).await.unwrap();
  let restarted = PlaybackJournal::new(directory.path());
  restarted.recover().await.unwrap();
  restarted.record(7, &end).await.unwrap();
  let ledger = restarted.ledger.lock().await;
  assert!(ledger.playing.is_empty());
  assert_eq!(ledger.outbox.len(), 2);
  assert_eq!(ledger.outbox[1].body["duration"], 19);
  assert_eq!(ledger.outbox[1].account, 7);
  assert_eq!(ledger.history.len(), 1);
}

#[test]
fn unknown_preparation_format_records_history_without_inventing_a_provider_format() {
  let mut ledger = fixture(false, None);
  for ticket in ledger.tickets.values_mut() {
    ticket.format = 0;
  }
  ledger
    .advance(7, &json!({"kind":"started","playback_id":"pending","resolution_id":"ticket"}))
    .unwrap();
  assert!(ledger.outbox.is_empty());
  assert_eq!(ledger.history.len(), 1);
}
