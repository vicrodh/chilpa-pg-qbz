//! The favourites the signed-in account keeps, as far as its lists were read.
//!
//! A list read to its end makes its kind known. A known kind answers for every
//! reference, so a page states the heart instead of leaving the host to guess.
//! A kind whose list is unread or partial stays unknown and is never stated.

use serde_json::Value;
use std::{
  collections::{HashMap, HashSet},
  sync::Mutex,
};

#[derive(Default)]
struct Listing {
  references: HashSet<String>,
  complete: bool,
}

#[derive(Default)]
struct Kept {
  account: u64,
  kinds: HashMap<String, Listing>,
}

#[derive(Default)]
pub(crate) struct FavoriteLedger {
  kept: Mutex<Kept>,
}

fn kind_of(reference: &str) -> &str {
  reference.split_once(':').map_or("", |(kind, _)| kind)
}

fn identity(reference: &str) -> &str {
  reference.split('@').next().unwrap_or(reference)
}

impl FavoriteLedger {
  fn open(&self, account: u64) -> std::sync::MutexGuard<'_, Kept> {
    let mut kept = self.kept.lock().unwrap_or_else(|poison| poison.into_inner());
    if kept.account != account {
      *kept = Kept { account, kinds: HashMap::new() };
    }
    kept
  }

  /// Takes one page of a kind's list. The first page starts the kind again.
  pub(crate) fn listed<'a>(
    &self,
    account: u64,
    kind: &str,
    offset: u64,
    references: impl Iterator<Item = &'a str>,
    complete: bool,
  ) {
    if account == 0 {
      return;
    }
    let mut kept = self.open(account);
    if offset == 0 {
      kept.kinds.remove(kind);
    }
    let Some(listing) = (match offset {
      0 => Some(kept.kinds.entry(kind.to_owned()).or_default()),
      _ => kept.kinds.get_mut(kind),
    }) else {
      return;
    };
    listing.references.extend(references.map(|reference| identity(reference).to_owned()));
    listing.complete = complete;
  }

  /// Applies a write the service accepted to a kind that is already known.
  pub(crate) fn changed(&self, account: u64, reference: &str, on: bool) {
    let mut kept = self.open(account);
    let reference = identity(reference);
    if let Some(listing) = kept.kinds.get_mut(kind_of(reference)) {
      if on {
        listing.references.insert(reference.to_owned());
      } else {
        listing.references.remove(reference);
      }
    }
  }

  pub(crate) fn status(&self, account: u64, reference: &str) -> Option<bool> {
    let kept = self.open(account);
    let reference = identity(reference);
    let listing = kept.kinds.get(kind_of(reference)).filter(|listing| listing.complete)?;
    Some(listing.references.contains(reference))
  }

  /// States the heart on a card when its kind is known.
  pub(crate) fn mark(&self, account: u64, card: Option<&mut Value>) {
    let Some(card) = card.filter(|card| card.is_object()) else {
      return;
    };
    let status = card["ref"].as_str().and_then(|reference| self.status(account, reference));
    if let Some(status) = status {
      card["favorite"] = status.into();
    }
  }
}

#[cfg(test)]
#[path = "checks/favorites_tests.rs"]
mod checks;
