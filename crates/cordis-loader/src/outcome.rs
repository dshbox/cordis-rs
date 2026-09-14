//! Complete immutable semantic outcomes for one Loader execution.
//!
//! [`LoadOutcome`] contains exactly one [`crate::outcome::EntryOutcome`] for every entry in the
//! executed [`crate::LoadPlan`], ordered parent-before-descendant with siblings
//! retaining declaration order. Structural, disabled, and pruned declarations
//! remain explicit rows rather than hidden skips. Plugin execution is partial:
//! one resolver or spawn failure becomes that entry's [`crate::outcome::EntryOutcome::Failed`]
//! and does not erase successful FiberHandles or stop later reachable entries.
//!
//! [`crate::EntryId`] is the exact declarative correlation identity. Resolve
//! keys are repeatable execution metadata, so duplicate keys stay separate
//! occurrences and this module deliberately provides no resolve-key lookup.

use cordis_core::FiberHandle;
use cordis_core::lifecycle::SpawnError;

use crate::plan::EntryId;
use crate::resolver::ResolverFailure;

/// The semantic result of executing one exact declarative plan entry.
///
/// Every plan entry contributes exactly one value. A descendant of a disabled
/// Plugin is always [`Pruned`](Self::Pruned), including structural groups and
/// separately disabled Plugins; otherwise a reachable group is [`Group`](Self::Group),
/// a reachable disabled Plugin is [`Disabled`](Self::Disabled), and a reachable
/// enabled Plugin is either [`Spawned`](Self::Spawned) or [`Failed`](Self::Failed).
#[derive(Debug)]
pub enum EntryOutcome {
    /// One reachable structural sequencing group; no Fiber is spawned for it.
    Group {
        /// Exact plan-entry correlation identity.
        id: EntryId,
    },
    /// One reachable disabled Plugin declaration.
    Disabled {
        /// Exact plan-entry correlation identity.
        id: EntryId,
    },
    /// One descendant removed from execution by a disabled Plugin ancestor.
    Pruned {
        /// Exact plan-entry correlation identity.
        id: EntryId,
        /// The reachable disabled Plugin whose subtree pruning covers this row.
        disabled_ancestor: EntryId,
    },
    /// One reachable Plugin successfully resolved and spawned.
    Spawned {
        /// Exact plan-entry correlation identity.
        id: EntryId,
        /// Exact `key`, falling back to `name`, used for this occurrence.
        resolve_key: String,
        /// Consumer control handle for this exact successful Fiber occurrence.
        fiber_handle: FiberHandle,
    },
    /// One reachable Plugin whose independent execution attempt failed.
    Failed {
        /// Exact plan-entry correlation identity.
        id: EntryId,
        /// Exact `key`, falling back to `name`, used for this occurrence.
        resolve_key: String,
        /// Failure normalized at the Loader phase boundary that owns it.
        failure: LoaderFailure,
    },
}

impl EntryOutcome {
    /// Exact plan-entry correlation identity carried by this outcome.
    pub fn id(&self) -> &EntryId {
        match self {
            Self::Group { id }
            | Self::Disabled { id }
            | Self::Pruned { id, .. }
            | Self::Spawned { id, .. }
            | Self::Failed { id, .. } => id,
        }
    }
}

/// Failure of one reachable Plugin entry during Loader execution.
///
/// Failures belong to their containing [`EntryOutcome::Failed`] and never abort
/// the whole load. Resolver errors are already normalized by the synchronous
/// resolver firewall; core spawn errors remain intact.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoaderFailure {
    /// The valid resolve key was unknown to the resolver.
    #[error("resolver did not recognize `{key}`")]
    UnresolvedKey {
        /// Exact resolve key for the failed occurrence.
        key: String,
    },
    /// Target resolution, preparation, overlay, or sealing failed.
    #[error("{0}")]
    Resolver(ResolverFailure),
    /// Core lifecycle admission or initial creation failed.
    #[error("{0}")]
    Spawn(SpawnError),
}

/// Complete immutable result of one partial [`crate::LoadPlan`] execution.
///
/// The ordered slice has exactly one row per plan entry. [`LoadOutcome::entry`]
/// performs exact [`EntryId`] correlation, [`LoadOutcome::fiber_handles`] yields every
/// successful FiberHandle in the same semantic execution order, and
/// [`LoadOutcome::is_ok`] is true exactly when no row is
/// [`EntryOutcome::Failed`]. Resolve keys may repeat; Loader intentionally has
/// no last-wins resolve-key projection. Construction is the Loader-to-caller
/// ownership handoff for successful FiberHandles; after delivery, dropping this value or
/// any contained FiberHandle is inert and lifecycle composition is caller policy.
#[derive(Debug)]
#[must_use = "LoadOutcome carries the caller's FiberHandle controls; inspect or retain it explicitly"]
pub struct LoadOutcome {
    entries: Vec<EntryOutcome>,
}

impl LoadOutcome {
    pub(crate) fn new(entries: Vec<EntryOutcome>) -> Self {
        Self { entries }
    }

    /// Every execution outcome in deterministic Loader order.
    pub fn entries(&self) -> &[EntryOutcome] {
        &self.entries
    }

    /// Look up the one outcome carrying this exact plan-lineage identity.
    pub fn entry(&self, id: &EntryId) -> Option<&EntryOutcome> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    /// Iterate successful FiberHandles in their outcome/spawn order.
    pub fn fiber_handles(&self) -> impl Iterator<Item = &FiberHandle> {
        self.entries.iter().filter_map(|entry| match entry {
            EntryOutcome::Spawned { fiber_handle, .. } => Some(fiber_handle),
            _ => None,
        })
    }

    /// Whether this execution contains no [`EntryOutcome::Failed`] row.
    pub fn is_ok(&self) -> bool {
        !self
            .entries
            .iter()
            .any(|entry| matches!(entry, EntryOutcome::Failed { .. }))
    }
}
