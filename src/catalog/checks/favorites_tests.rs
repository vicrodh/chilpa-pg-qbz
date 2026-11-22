use super::*;
use serde_json::json;

#[test]
fn test_a_kind_is_known_only_after_its_last_page() {
  let ledger = FavoriteLedger::default();
  assert_eq!(ledger.status(7, "album:1"), None, "An unread list must not state a heart");
  ledger.listed(7, "album", 0, ["album:1"].into_iter(), false);
  assert_eq!(ledger.status(7, "album:1"), None, "A partial list must not state a heart");
  ledger.listed(7, "album", 1, ["album:2"].into_iter(), true);
  assert_eq!(ledger.status(7, "album:1"), Some(true));
  assert_eq!(ledger.status(7, "album:2"), Some(true));
  assert_eq!(ledger.status(7, "album:3"), Some(false));
  assert_eq!(ledger.status(7, "artist:1"), None, "Another kind stays unknown");
}

#[test]
fn test_a_first_page_replaces_the_previous_list() {
  let ledger = FavoriteLedger::default();
  ledger.listed(7, "album", 0, ["album:1"].into_iter(), true);
  ledger.listed(7, "album", 0, ["album:2"].into_iter(), true);
  assert_eq!(ledger.status(7, "album:1"), Some(false));
  assert_eq!(ledger.status(7, "album:2"), Some(true));
}

#[test]
fn test_a_later_page_without_its_first_page_is_ignored() {
  let ledger = FavoriteLedger::default();
  ledger.listed(7, "album", 500, ["album:1"].into_iter(), true);
  assert_eq!(ledger.status(7, "album:1"), None);
}

#[test]
fn test_an_accepted_write_changes_a_known_kind_only() {
  let ledger = FavoriteLedger::default();
  ledger.changed(7, "album:1", true);
  assert_eq!(ledger.status(7, "album:1"), None, "A write must not make an unread kind known");
  ledger.listed(7, "track", 0, std::iter::empty(), true);
  ledger.changed(7, "track:5@playlist:9@entry:3", true);
  assert_eq!(ledger.status(7, "track:5"), Some(true), "Playback context is not identity");
  ledger.changed(7, "track:5", false);
  assert_eq!(ledger.status(7, "track:5@playlist:9"), Some(false));
}

#[test]
fn test_another_account_starts_unknown() {
  let ledger = FavoriteLedger::default();
  ledger.listed(7, "album", 0, ["album:1"].into_iter(), true);
  assert_eq!(ledger.status(8, "album:1"), None);
  assert_eq!(ledger.status(7, "album:1"), None, "The previous account's list was dropped");
  ledger.listed(0, "album", 0, ["album:1"].into_iter(), true);
  assert_eq!(ledger.status(0, "album:1"), None, "A signed-out read keeps nothing");
}

#[test]
fn test_a_card_states_its_heart_only_when_known() {
  let ledger = FavoriteLedger::default();
  let mut card = json!({"kind":"album","ref":"album:1"});
  ledger.mark(7, Some(&mut card));
  assert!(card.get("favorite").is_none());
  ledger.listed(7, "album", 0, ["album:1"].into_iter(), true);
  ledger.mark(7, Some(&mut card));
  assert_eq!(card["favorite"], true);
  let mut other = json!({"kind":"album","ref":"album:2"});
  ledger.mark(7, Some(&mut other));
  assert_eq!(other["favorite"], false);
  let mut absent = json!(null);
  ledger.mark(7, Some(&mut absent));
  assert!(absent.is_null());
  ledger.mark(7, None);
}
