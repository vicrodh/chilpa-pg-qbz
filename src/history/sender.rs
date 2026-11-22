use super::PlaybackJournal;
use crate::files;
use qobuz_api::{ApiError, QobuzClient};

impl PlaybackJournal {
  pub async fn flush(&self, api: &QobuzClient) {
    let Ok(_owner) = self.sender.try_lock() else {
      return;
    };
    let Some(identity) = api.session().await else {
      return;
    };
    let envelope = {
      let ledger = self.ledger.lock().await;
      ledger
        .outbox
        .front()
        .filter(|item| {
          item.account == identity.user_id && item.not_before <= chrono::Utc::now().timestamp()
        })
        .cloned()
    };
    let Some(envelope) = envelope else {
      return;
    };
    let batch = std::slice::from_ref(&envelope.body);
    let outcome = if envelope.opening {
      api.report_streaming_start(batch).await
    } else {
      api.report_streaming_end(batch, concat!("pg-qbz/", env!("CARGO_PKG_VERSION"))).await
    };
    let mut committed = self.ledger.lock().await;
    if committed.outbox.front().is_none_or(|item| item.key != envelope.key) {
      return;
    }
    let mut transaction = committed.clone();
    match outcome {
      Ok(()) => {
        transaction.outbox.pop_front();
        transaction.last_error = None;
      }
      Err(error) => {
        let head = transaction.outbox.front_mut().expect("outbox head");
        head.attempts = head.attempts.saturating_add(1);
        let delay = match error {
          ApiError::RateLimited(seconds) => seconds.clamp(5, 3600),
          _ => (5u64 << head.attempts.min(6)).min(300),
        };
        head.not_before = chrono::Utc::now().timestamp() + delay as i64;
        transaction.last_error = Some(error.to_string());
      }
    }
    match files::store_json(&self.file, &transaction) {
      Ok(()) => *committed = transaction,
      Err(error) => committed.last_error = Some(error),
    }
  }
}
