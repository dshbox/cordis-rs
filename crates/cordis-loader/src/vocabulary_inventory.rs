//! In-crate exhaustive inventories of the Loader vocabularies that are
//! `#[non_exhaustive]` for downstream crates.
//!
//! Downstream matches must carry a wildcard arm, so the compiler no longer
//! guards these inventories there. Inside this crate `#[non_exhaustive]` has no
//! effect, so each `match` below fails to compile when a variant is added or
//! removed without a deliberate update here. Field patterns use `..` on
//! purpose: only the variant inventory is normative.

use crate::outcome::EntryOutcome;
use crate::plan::RealmPolicy;
use crate::resolver::ResolverFailureKind;

fn entry_outcome_kind(outcome: &EntryOutcome) -> &'static str {
    match outcome {
        EntryOutcome::Group { .. } => "Group",
        EntryOutcome::Disabled { .. } => "Disabled",
        EntryOutcome::Pruned { .. } => "Pruned",
        EntryOutcome::Spawned { .. } => "Spawned",
        EntryOutcome::Failed { .. } => "Failed",
    }
}

fn realm_policy_kind(policy: &RealmPolicy) -> &'static str {
    match policy {
        RealmPolicy::Private => "Private",
        RealmPolicy::Shared { .. } => "Shared",
    }
}

fn resolver_failure_kind(kind: ResolverFailureKind) -> &'static str {
    match kind {
        ResolverFailureKind::ReturnedError => "ReturnedError",
        ResolverFailureKind::Panic => "Panic",
    }
}

#[test]
fn entry_outcome_has_exactly_five_kinds() {
    // An `EntryOutcome::Spawned` needs a live Fiber, so the exhaustive match is
    // the guard here; binding it to a fn pointer keeps it compiled and used.
    let inventory: fn(&EntryOutcome) -> &'static str = entry_outcome_kind;
    let _ = inventory;
}

#[test]
fn realm_policy_and_resolver_failure_kind_keep_their_inventories() {
    assert_eq!(
        [
            RealmPolicy::Private,
            RealmPolicy::Shared {
                label: "inventory".to_owned()
            },
        ]
        .iter()
        .map(realm_policy_kind)
        .collect::<Vec<_>>(),
        ["Private", "Shared"]
    );
    assert_eq!(
        [
            ResolverFailureKind::ReturnedError,
            ResolverFailureKind::Panic
        ]
        .map(resolver_failure_kind),
        ["ReturnedError", "Panic"]
    );
}
