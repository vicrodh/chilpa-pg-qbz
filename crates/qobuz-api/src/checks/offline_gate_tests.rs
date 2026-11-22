use super::*;

#[test]
fn test_gate_toggles() {
  let _lock = test_lock().lock().unwrap_or_else(|p| p.into_inner());
  let _reset = TestGateReset;
  set_offline(false);
  assert!(!is_offline());
  set_offline(true);
  assert!(is_offline());
  set_offline(false);
  assert!(!is_offline());
}
