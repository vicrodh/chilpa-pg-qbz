use super::*;
use crate::signing::{generate_signature, sign_get_file_url, sign_get_file_url_download};

// ── §12-1: the requests must be byte-identical to the reference ──────────
//
// These assert the SIGNATURE PREIMAGE and the endpoint paths, not just that
// "a signature is produced". The purchase path is the one feature nobody on
// this team can smoke-test — Qobuz Purchases is not sold in the owner's
// region — so inspection is not evidence and these tests are the only thing
// standing between a wrong request and a shipped, invisible failure.

/// The preimage has NO separators anywhere. The playback grant signs
/// `intentstream`; the purchase grant signs `intentdownload`. Both facts
/// were measured on the wire — the second one live on 2026-09-01 against a
/// DSD128 entitlement, where `stream` answers 400 and `download` answers the
/// file.
#[test]
fn test_file_url_signature_preimage_is_exact() {
  let track_id = 123_456_u64;
  let format_id = 27_u32;
  let timestamp = 1_700_000_000_u64;
  let secret = "0123456789abcdef0123456789abcdef";

  // Assembled here by hand, character for character, from the contract:
  //   trackgetFileUrl + format_id{fid} + intentstream + track_id{tid}
  //   + {timestamp} + {secret}
  let expected_preimage =
    format!("trackgetFileUrlformat_id{format_id}intentstreamtrack_id{track_id}{timestamp}{secret}");
  let expected = {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(expected_preimage.as_bytes());
    format!("{:x}", hasher.finalize())
  };

  assert_eq!(
    sign_get_file_url(track_id, format_id, timestamp, secret),
    expected,
    "the getFileUrl stream signature preimage drifted; playback will fail auth"
  );
}

/// The purchase grant: `intentdownload` in the same unseparated shape.
/// This is the signature that fetched a real DSF on 2026-09-01.
#[test]
fn test_purchase_file_url_signature_preimage_is_exact() {
  let track_id = 123_456_u64;
  let format_id = 56_u32;
  let timestamp = 1_700_000_000_u64;
  let secret = "0123456789abcdef0123456789abcdef";
  let expected_preimage = format!(
    "trackgetFileUrlformat_id{format_id}intentdownloadtrack_id{track_id}{timestamp}{secret}"
  );
  let expected = {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(expected_preimage.as_bytes());
    format!("{:x}", hasher.finalize())
  };
  assert_eq!(
    sign_get_file_url_download(track_id, format_id, timestamp, secret),
    expected,
    "the getFileUrl download signature preimage drifted; every purchase download \
             will answer HTTP 400"
  );
  assert_eq!(FileUrlIntent::Download.wire(), "download");
  assert_eq!(FileUrlIntent::Stream.wire(), "stream");
}

/// Guard the halves of the preimage a well-meaning edit would "fix": the
/// two intents never collide, and there are no separators.
#[test]
fn test_file_url_signatures_keep_intents_apart_and_carry_no_separators() {
  let (tid, fid, ts, secret) = (99_u64, 6_u32, 1_u64, "s");
  let stream = sign_get_file_url(tid, fid, ts, secret);
  let download = sign_get_file_url_download(tid, fid, ts, secret);
  assert_ne!(stream, download, "intent is inside the preimage");

  let with_separators = generate_signature(
    "trackgetFileUrl",
    &format!("format_id{fid}&intentstream&track_id{tid}"),
    ts,
    secret,
  );
  assert_ne!(stream, with_separators, "the preimage carries no separators");
}

/// The endpoint constants themselves — a typo here fails silently into an
/// empty purchases list, because the list endpoints never check status.
#[test]
fn test_purchase_endpoint_paths_are_the_reference_paths() {
  assert_eq!(paths::PURCHASE_GET_USER_PURCHASES, "/purchase/getUserPurchases");
  assert_eq!(paths::PURCHASE_GET_USER_PURCHASES_IDS, "/purchase/getUserPurchasesIds");
  assert_eq!(paths::ALBUM_GET, "/album/get");
  assert_eq!(paths::TRACK_GET, "/track/get");
}

// ── §12-2: the pagination loop's terminating condition ───────────────────

#[test]
fn test_pagination_stops_on_an_empty_page() {
  assert_eq!(purchase_page_step(0, 0, 1_000), PurchasePageStep::Stop);
  // Even mid-walk, and even when the server claims more remain.
  assert_eq!(purchase_page_step(500, 0, 1_000), PurchasePageStep::Stop);
}

#[test]
fn test_pagination_walks_while_items_remain() {
  assert_eq!(purchase_page_step(0, 500, 1_200), PurchasePageStep::Continue { next_offset: 500 });
  assert_eq!(
    purchase_page_step(500, 500, 1_200),
    PurchasePageStep::Continue { next_offset: 1_000 }
  );
  // Final page: 1000 + 200 >= 1200.
  assert_eq!(purchase_page_step(1_000, 200, 1_200), PurchasePageStep::Stop);
}

/// THE case from §2.3. A zero total stops the walk after one page even
/// though that page was full. The caller then reports the accumulated
/// length, so the symptom is "my library is exactly one page long", not
/// "truncated at 500" — a distinction that has to survive in a test because
/// it cannot be reproduced against a live account here.
#[test]
fn test_pagination_zero_total_stops_after_one_page_even_when_full() {
  assert_eq!(purchase_page_step(0, 500, 0), PurchasePageStep::Stop);
  assert_eq!(purchase_page_step(0, 1, 0), PurchasePageStep::Stop);
}

/// Exact-boundary and overshoot: `>=`, never `>`.
#[test]
fn test_pagination_boundary_is_inclusive() {
  assert_eq!(purchase_page_step(0, 500, 500), PurchasePageStep::Stop);
  assert_eq!(purchase_page_step(0, 501, 500), PurchasePageStep::Stop);
  assert_eq!(purchase_page_step(0, 499, 500), PurchasePageStep::Continue { next_offset: 499 });
}
