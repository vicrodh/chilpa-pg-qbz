use super::*;

// ── §2.6: the `streamable` split ─────────────────────────────────────────
//
// Two structs, same field name, three semantics. `PurchaseTrack.streamable`
// is a `bool` defaulting TRUE (`serde_true`) — the purchases endpoints are
// terse and that screen gates click-to-play on the flag, so a default of
// false would make every purchased album unclickable. Catalog
// `Track.streamable` is now `Option<bool>`: it does not guess in EITHER
// direction, because the greyout and the queue filter key on it and a wrong
// guess is visible on every screen in the app. Absence is resolved in
// exactly one function, `Track::is_streamable`, and nowhere else.

/// A purchases-list track defaults `streamable` to **true**.
#[test]
fn test_purchase_track_streamable_defaults_true() {
  let t: PurchaseTrack = serde_json::from_str(r#"{"id":1,"title":"T"}"#).unwrap();
  assert!(t.streamable, "PurchaseTrack.streamable defaults TRUE (serde_true)");
}

/// A CATALOG track that does not mention `streamable` deserializes to
/// `None`, and `None` reads as **available**.
///
/// This is the regression that would otherwise grey out the whole
/// catalogue. While the field was a `bool` with a bare `#[serde(default)]`,
/// an endpoint that omits the key produced `false` — "pulled from Qobuz" —
/// for every track it returned. `/album/get` does send the key (captured
/// 2026-08-17), but `/playlist/get?extra=tracks` never has been, and that is
/// the surface where dead tracks actually show up.
#[test]
fn test_catalog_track_streamable_absent_is_none_and_reads_available() {
  let t: Track = serde_json::from_str(r#"{"id":1,"title":"T"}"#).unwrap();
  assert_eq!(
    t.streamable, None,
    "an absent key must stay absent — never collapse into Some(false)"
  );
  assert!(t.is_streamable(), "absence is NOT a claim of unavailability: absent reads as available");
}

/// An explicit `"streamable": false` is honoured, and it is the ONLY shape
/// that marks a track as pulled.
///
/// This is the real wire signal, captured from
/// `/album/get?album_id=0886443985094` on 2026-08-17: track 1 carries
/// `false` (and keeps its `isrc`, which is what makes the replacement
/// search's ISRC short-circuit reachable) while the album around it is
/// `true`.
#[test]
fn test_catalog_track_streamable_false_is_honoured() {
  let t: Track = serde_json::from_str(r#"{"id":1,"title":"T","streamable":false}"#).unwrap();
  assert_eq!(t.streamable, Some(false));
  assert!(!t.is_streamable(), "Some(false) is the ONLY shape that marks a track unavailable");
}

#[test]
fn test_catalog_album_streamable_absent_reads_available() {
  let album: Album = serde_json::from_str(r#"{"id":"a","title":"A"}"#).unwrap();
  assert_eq!(album.streamable, None);
  assert!(album.is_streamable());
}

#[test]
fn test_catalog_album_streamable_false_is_honoured() {
  let album: Album = serde_json::from_str(r#"{"id":"a","title":"A","streamable":false}"#).unwrap();
  assert_eq!(album.streamable, Some(false));
  assert!(!album.is_streamable());
}

#[test]
fn test_embedded_album_availability_keeps_unknown_distinct_from_false() {
  let absent: AlbumSummary = serde_json::from_str(r#"{"id":"a","title":"A"}"#).unwrap();
  assert_eq!(absent.streamable, None);
  assert!(absent.is_streamable());

  let withdrawn: AlbumSummary =
    serde_json::from_str(r#"{"id":"a","title":"A","version":"Remastered","streamable":false}"#)
      .unwrap();
  assert_eq!(withdrawn.streamable, Some(false));
  assert!(!withdrawn.is_streamable());
  assert!(!withdrawn.is_upcoming());
  assert_eq!(withdrawn.version.as_deref(), Some("Remastered"));
}

#[test]
fn test_embedded_future_album_is_upcoming_not_withdrawn() {
  let upcoming: AlbumSummary = serde_json::from_str(
    r#"{"id":"a","title":"A","streamable":false,"release_date_stream":"2999-01-01"}"#,
  )
  .unwrap();
  assert!(upcoming.is_upcoming());
}

/// `downloadable` drives three list behaviours (the hide-unavailable filter,
/// the album click gate, the unavailable marker), so its default is not
/// decoration: getting it wrong ships clickable unavailable albums.
#[test]
fn test_purchase_album_downloadable_defaults_true() {
  let a: PurchaseAlbum = serde_json::from_str(r#"{"id":"x","title":"A"}"#).unwrap();
  assert!(a.downloadable);
  assert!(!a.downloaded, "downloaded is local-only, never from the wire");
  assert!(a.tracks.is_none());
  assert!(a.purchased_at.is_none());
}

// ── §2.5: leniency is load-bearing, and has a documented limit ────────────

/// Wrong-typed optional fields degrade to `None` instead of failing the item.
#[test]
fn test_wrong_typed_optionals_degrade_to_none() {
  let json = r#"{
            "id": 7, "title": "T",
            "media_number": "not a number",
            "maximum_sampling_rate": {"nope": true},
            "maximum_bit_depth": [],
            "album": 12345,
            "purchased_at": "yesterday",
            "version": []
        }"#;
  let t: PurchaseTrack = serde_json::from_str(json).unwrap();
  assert_eq!(t.id, 7);
  assert_eq!(t.media_number, None);
  assert_eq!(t.maximum_sampling_rate, None);
  assert_eq!(t.maximum_bit_depth, None);
  assert!(t.album.is_none());
  assert_eq!(t.purchased_at, None);
  assert_eq!(t.version, None);
}

/// A malformed page collapses to an EMPTY page while its sibling still
/// parses. This is why a 401/403 body shaped like JSON and "you own nothing"
/// are indistinguishable — stated here so nobody later mistakes the empty
/// state for proof that the request succeeded.
#[test]
fn test_malformed_page_empties_itself_without_taking_its_sibling() {
  let json = r#"{
            "albums": "this is not a page",
            "tracks": {"offset":0,"limit":50,"total":1,"items":[{"id":9,"title":"Nine"}]}
        }"#;
  let r: PurchaseResponse = serde_json::from_str(json).unwrap();
  assert!(r.albums.items.is_empty());
  assert_eq!(r.albums.total, 0);
  assert_eq!(r.tracks.items.len(), 1);
  assert_eq!(r.tracks.items[0].title, "Nine");
}

/// §2.5b-2, measured against a live account: `?type=albums` OMITS the
/// `tracks` key entirely — it is not present-and-zero. A port that models a
/// present-but-empty sibling page is modelling the wrong thing.
#[test]
fn test_typed_response_with_an_absent_sibling_page_parses() {
  let json = r#"{
            "albums": {"offset":0,"limit":500,"total":0,"items":[]},
            "user": {"id": 1, "login": "someone"}
        }"#;
  let r: PurchaseResponse = serde_json::from_str(json).unwrap();
  assert!(r.albums.items.is_empty());
  assert!(r.tracks.items.is_empty(), "absent sibling defaults, never fails");
  assert_eq!(r.tracks.total, 0);
}

/// §2.5b-3: the IDS envelope has a DIFFERENT page shape — `{total, items}`
/// with no `offset` and no `limit`. If those scalars were required, the
/// lenient page wrapper would swallow the failure and hand back an empty
/// page, and both tab counters would read 0 forever with nothing logged.
#[test]
fn test_ids_response_page_shape_preserves_total_without_offset_or_limit() {
  let json = r#"{
            "albums": {"total": 42, "items": [1,2,3]},
            "tracks": {"total": 7, "items": []},
            "user": {"id": 1, "login": "someone"}
        }"#;
  let r: PurchaseIdsResponse = serde_json::from_str(json).unwrap();
  assert_eq!(r.albums.total, 42, "the tab counter comes from here");
  assert_eq!(r.tracks.total, 7);
  assert_eq!(r.albums.offset, 0);
  assert_eq!(r.albums.limit, 0);
}

/// The documented LIMIT of the leniency: a non-object top level still fails.
/// Recorded so the empty-list-on-everything claim is not overstated.
/// The documented LIMIT of the leniency — MEASURED 2026-08-16, and it is
/// wider than the contract stated.
///
/// The contract (§2.5) says "a transport error, a non-JSON body and an
/// invalid top-level shape still FAIL". The first two hold. The third does
/// not, for ARRAYS: serde's derived `Deserialize` accepts a struct from a
/// JSON sequence as well as a map, and since both fields carry
/// `#[serde(default)]` plus a lenient page wrapper, `[]` — and even
/// `[1, 2]`, whose elements are consumed positionally and swallowed —
/// deserialize into a perfectly valid EMPTY response.
///
/// What genuinely fails is a SCALAR top level: string, number, bool, null.
///
/// This does not change the contract's conclusion (a JSON-shaped 401/403
/// body is an object, and it collapses to "you own nothing" exactly as
/// described) but it does widen the silent-empty surface by one shape, and
/// on a feature nobody can smoke-test the exact boundary is worth pinning
/// down rather than approximating.
#[test]
fn test_only_scalar_top_levels_fail_arrays_collapse_to_empty() {
  for ok in ["{}", "[]", "[1,2]"] {
    let parsed = serde_json::from_str::<PurchaseResponse>(ok)
      .unwrap_or_else(|e| panic!("expected {ok} to collapse to empty, got {e}"));
    assert!(parsed.albums.items.is_empty());
    assert!(parsed.tracks.items.is_empty());
  }

  for err in ["\"nope\"", "7", "null", "true"] {
    assert!(
      serde_json::from_str::<PurchaseResponse>(err).is_err(),
      "a scalar top level must still fail: {err}"
    );
  }
}

// ── §10-C: the version field that closes the latent #360 regression ───────

#[test]
fn test_purchase_track_carries_a_version_when_the_catalog_supplies_one() {
  let t: PurchaseTrack =
    serde_json::from_str(r#"{"id":1,"title":"Song","version":"Live"}"#).unwrap();
  assert_eq!(t.version.as_deref(), Some("Live"));
}

// ── §14.3: goodies must never take an album down ──────────────────────────

/// The shape of a POPULATED goodies list has never been observed — it comes
/// back empty on every album nobody owns. So an unexpected shape has to
/// degrade to "no goodies", not to "this album will not open". `/album/get`
/// is parsed with the strict structs, which is exactly where that would bite.
#[test]
fn test_surprising_goodies_shape_does_not_fail_the_album() {
  for weird in [
    r#"{"id":"1","title":"A","goodies":"a string"}"#,
    r#"{"id":"1","title":"A","goodies":{"unexpected":"object"}}"#,
    r#"{"id":"1","title":"A","goodies":[{"totally":"different"}]}"#,
    r#"{"id":"1","title":"A","goodies":7}"#,
  ] {
    let album: Album = serde_json::from_str(weird)
      .unwrap_or_else(|e| panic!("goodies shape took the album down: {weird} → {e}"));
    assert_eq!(album.title, "A");
  }
}

#[test]
fn test_well_formed_goodie_parses_and_reads_defensively() {
  let album: Album = serde_json::from_str(
    r#"{"id":"1","title":"A","goodies":[
                {"id":5,"name":"Booklet","url":"https://u/b.pdf","original_url":"https://o/b.pdf"}
            ]}"#,
  )
  .unwrap();
  let goodies = album.goodies.expect("goodies present");
  assert_eq!(goodies.len(), 1);
  assert_eq!(goodies[0].best_url(), Some("https://o/b.pdf"));
  assert_eq!(goodies[0].display_name(), "Booklet");
}
