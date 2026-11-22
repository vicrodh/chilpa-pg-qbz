use super::*;
#[cfg(test)]
#[path = "checks/tests_tests.rs"]
mod checks;

// ─── §12-4: purchase deserializers, one payload per missing optional field ────
//
// The whole point of this module is that the purchase path cannot be smoke-
// tested: Qobuz Purchases is not sold in the owner's region, so the only
// populated payloads anyone will ever see belong to end users. Every default
// below is therefore asserted rather than assumed, and the ones that DIFFER
// between two structs with the same field name get their own test, because that
// is the trap most likely to invert a screen's behaviour silently.
#[cfg(test)]
#[path = "checks/tests_purchase_deserializer_tests.rs"]
mod checks_purchase_deserializer_tests;

#[cfg(test)]
#[path = "checks/tests_upcoming_tests.rs"]
mod checks_upcoming_tests;
