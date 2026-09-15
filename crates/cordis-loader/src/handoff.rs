//! Private ownership bridge from core FiberHandle handoff to caller outcome delivery.
//!
//! Abandoned pre-delivery successes roll back through core's framework-owned
//! completion seam, so executor shutdown cannot silently discard the reverse-order
//! disposal obligation.

use crate::outcome::{EntryOutcome, LoadOutcome};

pub(crate) struct ResultHandoff {
    entries: Option<Vec<EntryOutcome>>,
}

impl ResultHandoff {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Some(Vec::with_capacity(capacity)),
        }
    }

    pub(crate) fn push(&mut self, outcome: EntryOutcome) {
        self.entries
            .as_mut()
            .expect("unfinished result handoff owns its entries")
            .push(outcome);
    }

    pub(crate) fn len(&self) -> usize {
        self.entries
            .as_ref()
            .expect("unfinished result handoff owns its entries")
            .len()
    }

    pub(crate) fn finish(mut self) -> LoadOutcome {
        let entries = self
            .entries
            .take()
            .expect("result handoff finishes exactly once");
        LoadOutcome::new(entries)
    }
}

/// If final delivery never happens, transfer every successful FiberHandle to detached
/// rollback before these private entries can fall through inert FiberHandle Drop.
impl Drop for ResultHandoff {
    fn drop(&mut self) {
        let Some(entries) = self.entries.take() else {
            return;
        };
        rollback(entries);
    }
}

fn rollback(entries: Vec<EntryOutcome>) {
    cordis_core::__internal::detach_completion(async move {
        for entry in entries.into_iter().rev() {
            let EntryOutcome::Spawned { fiber_handle, .. } = entry else {
                continue;
            };
            let _ = fiber_handle.dispose().await;
        }
    });
}
