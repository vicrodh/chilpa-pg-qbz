use super::*;
use crate::library_index::{IndexedItem, Membership};

fn card(id: usize) -> Value {
  json!({"kind":"award","ref":format!("award:{id}"),"title":format!("Award {id:04}")})
}
fn status() -> IndexStatus {
  IndexStatus { account: 7, generation: 1, revision: 1, checked_at: 1, items: 660 }
}
fn following() -> Query {
  Query { membership_groups: vec![vec![Membership::Followed]], ..Default::default() }
}

#[test]
fn all_awards_are_not_limited_to_followed_memberships() {
  let catalog = (0..19).map(card).collect();
  let mut saved = card(3);
  saved["added_at"] = 42.into();
  let (page, total) = select(status(), catalog, vec![saved], &Query::default(), None, 100).unwrap();
  assert_eq!(total, 19);
  assert_eq!(page.items.len(), 19);
  assert_eq!(page.items.iter().filter(|row| row["favorite"] == true).count(), 1);
  assert!(page.items.iter().any(|row| row["ref"] == "award:3" && row["followed"] == true));
  assert!(page.items.iter().all(|row| row["art_fit"] == "contain"));
}

#[test]
fn followed_filter_and_favorite_alias_show_only_saved_awards() {
  for membership in [Membership::Followed, Membership::Favorites] {
    let query = Query { membership_groups: vec![vec![membership]], ..Default::default() };
    let (page, total) =
      select(status(), (0..19).map(card).collect(), vec![card(3)], &query, None, 100).unwrap();
    assert_eq!(total, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0]["ref"], "award:3");
  }
}

#[test]
fn complete_catalog_pages_do_not_repeat_and_queries_invalidate_old_cursors() {
  let query = Query { order: Order::Title, ..Default::default() };
  let (one, total) =
    select(status(), (0..205).map(card).collect(), vec![], &query, None, 100).unwrap();
  assert_eq!(total, 205);
  assert_eq!(one.items.len(), 100);
  let (two, _) =
    select(status(), (0..205).map(card).collect(), vec![], &query, one.next.as_deref(), 100)
      .unwrap();
  let (three, _) =
    select(status(), (0..205).map(card).collect(), vec![], &query, two.next.as_deref(), 100)
      .unwrap();
  assert_eq!(two.items.len(), 100);
  assert_eq!(three.items.len(), 5);
  assert!(!three.has_more);
  assert_eq!(one.items[0]["ref"], "award:0");
  assert_eq!(two.items[0]["ref"], "award:100");
  assert!(select(
    status(),
    (0..205).map(card).collect(),
    vec![],
    &following(),
    one.next.as_deref(),
    100
  )
  .is_err());
  let mut another = status();
  another.account = 8;
  assert!(select(another, (0..205).map(card).collect(), vec![], &query, one.next.as_deref(), 100)
    .is_err());
}

#[test]
fn search_count_sorting_and_membership_changes_use_current_results() {
  let query = Query { text: "Award 0003".into(), order: Order::Title, ..Default::default() };
  let (page, total) =
    select(status(), (0..19).map(card).collect(), vec![], &query, None, 100).unwrap();
  assert_eq!(total, 1);
  assert_eq!(page.items[0]["ref"], "award:3");
  let mut saved = card(10);
  saved["added_at"] = 7.into();
  let (page, _) =
    select(status(), (0..19).map(card).collect(), vec![saved.clone()], &Query::default(), None, 1)
      .unwrap();
  assert_eq!(page.items[0]["ref"], "award:10");
  assert!(select(
    status(),
    (0..19).map(card).collect(),
    vec![],
    &Query::default(),
    page.next.as_deref(),
    1
  )
  .is_err());
  let (page, total) = select(status(), vec![], vec![saved], &following(), None, 100).unwrap();
  assert_eq!(total, 1);
  assert!(page.items[0]["favorite"] == true);
}

#[test]
fn catalog_cache_survives_restart_and_never_adds_unfollowed_awards_to_library_all() {
  let directory = tempfile::tempdir().unwrap();
  {
    let index = LibraryIndex::open(directory.path(), 7).unwrap();
    let generation = index.begin_generation(&[Membership::Followed], 1).unwrap();
    let saved = card(3);
    index
      .ingest(
        generation,
        Membership::Followed,
        &[IndexedItem { item: &saved, added_at: 42, explicit: true }],
      )
      .unwrap();
    index.finish_collection(generation, Membership::Followed).unwrap();
    index.publish(generation, 2).unwrap();
    index.remember_awards(&(0..19).map(card).collect::<Vec<_>>(), 10).unwrap();
    let revision = index.award_stamp().unwrap().revision;
    index.remember_awards(&(0..19).map(card).collect::<Vec<_>>(), 11).unwrap();
    assert_eq!(
      index.award_stamp().unwrap().revision,
      revision,
      "Equal catalog refresh keeps its revision"
    );
    index.defer_awards(12).unwrap();
    assert!(index.award_stamp().unwrap().available);
    assert_eq!(
      index
        .query(&Query { explicit_only: true, ..Default::default() }, None, 100)
        .unwrap()
        .items
        .len(),
      1
    );
  }
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  let (status, catalog, saved, _) = index.award_snapshot().unwrap();
  let (page, total) = select(status, catalog.unwrap(), saved, &following(), None, 100).unwrap();
  assert_eq!(total, 1);
  assert_eq!(page.items[0]["ref"], "award:3");
  assert_eq!(index.award_stamp().unwrap().retry_at, 72);
}

#[test]
fn catalog_limits_reject_oversized_snapshots_without_replacing_last_good_data() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  index.remember_awards(&[card(1)], 1).unwrap();
  assert!(index.remember_awards(&(0..=MAX_AWARDS).map(card).collect::<Vec<_>>(), 2).is_err());
  assert!(index.remember_awards(&[json!({"title":"x".repeat(MAX_BYTES)})], 2).is_err());
  assert_eq!(index.award_snapshot().unwrap().1.unwrap()[0]["ref"], "award:1");
}

#[tokio::test]
async fn offline_catalog_read_keeps_all_awards_and_filtered_counts_after_restart() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  index.remember_awards(&(0..19).map(card).collect::<Vec<_>>(), 1).unwrap();
  drop(index);
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  let api = qobuz_api::QobuzClient::new().unwrap();
  let page = index.awards_page(&api, &Query::default(), None, 100, false, false).await.unwrap();
  assert_eq!(page.total, 19);
  assert_eq!(page.page.items.len(), 19);
  assert!(page.stale);
  let page = index.awards_page(&api, &following(), None, 100, false, false).await.unwrap();
  assert_eq!(page.total, 0);
  assert!(page.page.items.is_empty());
}

#[tokio::test]
async fn missing_catalog_does_not_masquerade_as_complete_when_offline_or_refresh_fails() {
  let directory = tempfile::tempdir().unwrap();
  let index = LibraryIndex::open(directory.path(), 7).unwrap();
  let api = qobuz_api::QobuzClient::new().unwrap();
  assert!(index.awards_page(&api, &Query::default(), None, 100, false, false).await.is_err());
  assert!(index.awards_page(&api, &Query::default(), None, 100, true, true).await.is_err());
  assert!(!index.award_stamp().unwrap().available);
  index.remember_awards(&[card(1)], 1).unwrap();
  let page = index.awards_page(&api, &Query::default(), None, 100, true, true).await.unwrap();
  assert_eq!(page.total, 1);
  assert!(page.error.is_some());
  assert!(!page.error.unwrap().contains("request_sig"));
}
