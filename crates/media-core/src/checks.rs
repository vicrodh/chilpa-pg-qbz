use super::*;

#[test]
fn test_counter_vector() {
  let mut bytes = [0; 32];
  ContentKey::from_bytes([0; 16]).apply(&[0; 8], &mut bytes);
  assert_eq!(
    bytes,
    [
      102, 233, 75, 212, 239, 138, 44, 59, 136, 76, 250, 89, 202, 52, 43, 46, 88, 226, 252, 206,
      250, 126, 48, 97, 54, 127, 29, 87, 164, 231, 69, 90
    ]
  );
}

#[test]
fn test_missing_and_partial_descriptors() {
  for bytes in [vec![], vec![0; 7], vec![0; 64], vec![255; 64]] {
    assert!(AudioIndex::read(&bytes).is_err());
    assert!(PacketIndex::read(&bytes).is_err());
  }
}

#[test]
fn test_rejected_keys() {
  for seed in ["1", "zz", "éé"] {
    assert!(SessionKey::derive(seed, ".").is_err());
  }
  assert!(SessionKey::derive("00", "missing").is_err());
  let key = SessionKey::derive("00", ".").unwrap();
  for envelope in ["", "qbz-1", "qbz-1..", "qbz-1.!.AA", "qbz-1.AA.AA"] {
    assert!(key.open(envelope).is_err());
  }
}
