use super::*;

#[test]
fn test_rfc5869_vector() {
  let salt: Vec<u8> = (0..=12).collect();
  let info: Vec<u8> = (240..=249).collect();
  let descriptor = format!("{}.{}", URL_SAFE_NO_PAD.encode(salt), URL_SAFE_NO_PAD.encode(info));
  let wrapping = SessionKey::derive(&"0b".repeat(22), &descriptor).unwrap();
  assert_eq!(wrapping.0, [60, 178, 95, 37, 250, 172, 213, 122, 144, 67, 79, 100, 208, 54, 47, 42]);
}
