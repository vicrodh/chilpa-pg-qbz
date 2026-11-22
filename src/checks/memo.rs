use super::*;
use serde_json::json;

#[test]
fn test_expiry_and_prefix_invalidation() {
  let store = ReplyMemo::default();
  for (key, lifetime) in [("mix daily", 60), ("mix weekly", 60), ("page home", 60), ("expired", 0)]
  {
    store.put(key, Duration::from_secs(lifetime), &json!(key));
  }
  store.remove_prefix("mix daily");
  for (key, present) in
    [("mix daily", false), ("mix weekly", true), ("page home", true), ("expired", false)]
  {
    assert_eq!(store.get(key).is_some(), present);
  }
  store.clear();
  assert!(store.get("page home").is_none());
}

#[tokio::test]
async fn test_parallel_reads_share_a_reservation() {
  let store = Arc::new(ReplyMemo::default());
  let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
  let operation = |store: Arc<ReplyMemo>, calls: Arc<std::sync::atomic::AtomicUsize>| async move {
    let _reservation = store.reserve("same").await;
    if store.get("same").is_none() {
      calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      tokio::task::yield_now().await;
      store.put("same", Duration::from_secs(60), &json!("result"));
    }
  };
  tokio::join!(operation(store.clone(), calls.clone()), operation(store.clone(), calls.clone()));
  assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}
