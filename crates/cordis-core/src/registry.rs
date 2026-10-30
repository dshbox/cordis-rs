//! Private Registry residency and exact allocation claims.

use crate::fiber::{Fiber, LifecycleRecursion};
use parking_lot::Mutex;
use std::any::TypeId;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(
    dead_code,
    reason = "anonymous allocation and bulk removal are exercised by internal lifecycle probes"
)]
pub(crate) enum PluginKey {
    Typed(TypeId),
    Anonymous(u64),
}

pub(crate) struct PluginGroup {
    id: PluginKey,
    fibers: Mutex<Vec<Arc<Fiber>>>,
}

impl PluginGroup {
    fn new(id: PluginKey) -> Arc<Self> {
        Arc::new(Self {
            id,
            fibers: Mutex::new(Vec::new()),
        })
    }
    fn fiber_count(&self) -> usize {
        self.fibers.lock().len()
    }
    fn take_fiber(&self, fiber: &Arc<Fiber>) -> (Option<Arc<Fiber>>, bool) {
        let mut fibers = self.fibers.lock();
        let removed = fibers
            .iter()
            .position(|candidate| Arc::ptr_eq(candidate, fiber))
            .map(|index| fibers.remove(index));
        (removed, fibers.is_empty())
    }
}

struct RegistryState {
    allocations: Mutex<HashMap<PluginKey, Arc<PluginGroup>>>,
    /// Flat strong residency authority. Current-allocation detach does not
    /// remove a Fiber; only its exact terminal residency release does.
    residents: Mutex<Vec<Arc<Fiber>>>,
    /// Serialized with attach by the allocations lock; terminal, never reopened.
    closed: AtomicBool,
    #[cfg(test)]
    admissions: AtomicUsize,
}

pub(crate) struct Registry {
    state: Arc<RegistryState>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(RegistryState {
                allocations: Mutex::new(HashMap::new()),
                residents: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
                #[cfg(test)]
                admissions: AtomicUsize::new(0),
            }),
        }
    }
    pub(crate) fn snapshot_fibers(&self) -> Vec<Arc<Fiber>> {
        // Serialize flat observation with membership publication.
        let _guard = self.state.allocations.lock();
        self.state.residents.lock().clone()
    }
    #[cfg(test)]
    pub(crate) fn admission_count(&self) -> usize {
        self.state.admissions.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    fn resident_fiber_count(&self) -> usize {
        let _guard = self.state.allocations.lock();
        self.state.residents.lock().len()
    }
    /// Atomically attach one Fiber and install its exact residency claim.
    ///
    /// Claim installation happens while the Registry mapping is locked and
    /// before membership becomes visible to detach. A remover therefore cannot
    /// freeze this Fiber until its ordinary terminal barrier can release the
    /// exact allocation occurrence. The claim itself is weak to avoid a cycle.
    pub(crate) fn attach_fiber(&self, key: PluginKey, fiber: Arc<Fiber>) -> bool {
        let mut allocations = self.state.allocations.lock();
        if self.state.closed.load(Ordering::SeqCst) {
            return false;
        }
        let allocation = allocations
            .entry(key)
            .or_insert_with(|| PluginGroup::new(key))
            .clone();
        let claim = ResidencyClaim {
            allocation: Arc::downgrade(&allocation),
            fiber: Arc::downgrade(&fiber),
            registry: Arc::downgrade(&self.state),
        };
        let previous = fiber.residency.lock().replace(claim);
        assert!(
            previous.is_none(),
            "a Fiber may have only one residency occurrence"
        );
        allocation.fibers.lock().push(fiber.clone());
        self.state.residents.lock().push(fiber);
        #[cfg(test)]
        self.state.admissions.fetch_add(1, Ordering::SeqCst);
        true
    }
    /// Close admission and freeze all residents at the same linearization point
    /// as attach, including Fibers detached by earlier typed removals.
    fn close(&self) -> Result<Vec<Arc<Fiber>>, LifecycleRecursion> {
        let _allocations = self.state.allocations.lock();
        let residents = self.state.residents.lock();
        crate::fiber::refuse_group_removal_recursion(&residents)?;
        self.state.closed.store(true, Ordering::SeqCst);
        Ok(residents.clone())
    }

    /// Atomically detach one current allocation and freeze its completion set.
    ///
    /// Attach uses the same Registry → allocation lock order, so a Fiber is
    /// either already in `fibers` when the mapping is detached or it observes
    /// the missing mapping and attaches to a fresh allocation. Recursion is
    /// checked while both facts are fixed and therefore refuses before detach.
    fn detach_for_removal(
        &self,
        key: PluginKey,
    ) -> std::result::Result<Option<DetachedGroup>, LifecycleRecursion> {
        let mut allocations = self.state.allocations.lock();
        let Some(allocation) = allocations.get(&key).cloned() else {
            return Ok(None);
        };
        let fibers = allocation.fibers.lock();
        crate::fiber::refuse_group_removal_recursion(&fibers)?;
        let frozen = fibers.clone();
        let detached = allocations
            .remove(&key)
            .expect("current allocation remains mapped while Registry lock is held");
        drop(fibers);
        Ok(Some(DetachedGroup {
            allocation: detached,
            fibers: frozen,
        }))
    }
}

/// One committed bulk-removal allocation. Keeping the allocation strongly alive
/// lets every frozen Fiber release its exact weak residency claim while the old
/// mapping is detached and a same-key replacement may already exist.
struct DetachedGroup {
    allocation: Arc<PluginGroup>,
    fibers: Vec<Arc<Fiber>>,
}

impl DetachedGroup {
    async fn dispose_all(self, root: &Arc<crate::context::Root>) {
        let Self { allocation, fibers } = self;
        for fiber in fibers {
            fiber.dispose().await;
            // Exact disposal and unlink have completed, but this frozen
            // snapshot can now be the last Arc retaining user Plugin input.
            // Contain its destructor separately so a panic cannot skip the
            // remaining members or suppress group completion.
            let logger = root.logger.logger_for_fiber(&fiber.name);
            crate::contained::contain("bulk removal member destruction", Some(&logger), || {
                drop(fiber)
            });
        }
        debug_assert_eq!(allocation.fiber_count(), 0);
    }
}

impl crate::Context {
    /// Terminally close this Runtime to new Fibers and await every resident
    /// Fiber's cleanup and exact unlink, including Fibers already detached by
    /// typed removal. The permanent root Fiber and its registrations remain.
    ///
    /// Admission closes under the same lock as residency attach: a racing spawn
    /// is either included in the barrier or refused with InactiveContext before
    /// commit. Typed removal and handle Drop cannot hide a resident from it.
    /// Completion remains owned by Cordis if this caller abandons the wait.
    ///
    /// A call from a resident Fiber's settle context is refused before closing
    /// admission, rather than waiting on itself. A pending creation still owned
    /// by a live spawn future must be completed or dropped by its caller; this
    /// operation never reports success before that Fiber reaches its barrier.
    pub async fn close_plugins(&self) -> Result<(), LifecycleRecursion> {
        let residents = self.root.registry.close()?;
        let report = crate::framework_task::PanicReport {
            logger: self.logger(),
            operation: "terminal plugin removal",
            context: "all resident Fibers".to_owned(),
        };
        let root = self.root.clone();
        crate::effect::CallerDriven::new(Some(report), async move {
            futures::future::join_all(residents.iter().map(|fiber| fiber.dispose())).await;
            for fiber in residents {
                let logger = root.logger.logger_for_fiber(&fiber.name);
                crate::contained::contain(
                    "terminal removal member destruction",
                    Some(&logger),
                    || drop(fiber),
                );
            }
        })
        .await;
        Ok(())
    }

    /// Remove the current allocation for typed Plugin `P`.
    ///
    /// Detach is the irreversible commit: members already attached are frozen
    /// into this removal, while later same-type spawns create or join a fresh
    /// allocation. After detach, disposal is framework-owned and reaches every
    /// member's ordinary terminal unlink barrier even if this caller is
    /// cancelled or keeps this future without polling it. An absent allocation
    /// is already removed. As with
    /// [`FiberHandle::dispose`](crate::FiberHandle::dispose), await this future
    /// rather than blocking a Tokio worker thread on it.
    ///
    /// A call from the settle context of a Fiber in the current allocation is
    /// refused before detach so it cannot synchronously wait on its own teardown.
    pub async fn remove_plugins<P: crate::Plugin>(
        &self,
    ) -> std::result::Result<(), LifecycleRecursion> {
        let detached = self
            .root
            .registry
            .detach_for_removal(PluginKey::Typed(TypeId::of::<P>()))?;
        let Some(detached) = detached else {
            return Ok(());
        };

        let report = crate::framework_task::PanicReport {
            logger: self.logger(),
            operation: "typed plugin removal",
            context: format!("plugin={}", std::any::type_name::<P>()),
        };
        let root = self.root.clone();
        crate::effect::CallerDriven::new(Some(report), async move {
            detached.dispose_all(&root).await;
        })
        .await;
        Ok(())
    }
}

pub(crate) struct ResidencyClaim {
    allocation: Weak<PluginGroup>,
    fiber: Weak<Fiber>,
    registry: Weak<RegistryState>,
}

impl ResidencyClaim {
    /// Release only the exact allocation represented by this claim.
    pub(crate) fn release(self) {
        let (Some(registry), Some(allocation), Some(fiber)) = (
            self.registry.upgrade(),
            self.allocation.upgrade(),
            self.fiber.upgrade(),
        ) else {
            return;
        };
        // Match attach's registry → allocation lock order. This is what
        // prevents release from deadlocking an attach racing terminal unlink.
        let (removed_group_fiber, removed_resident, removed_allocation) = {
            let mut allocations = registry.allocations.lock();
            let current = allocations
                .get(&allocation.id)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, &allocation));
            let (removed_group_fiber, idle) = allocation.take_fiber(&fiber);
            let removed_allocation = if current && idle {
                allocations.remove(&allocation.id)
            } else {
                None
            };
            let removed_resident = {
                let mut residents = registry.residents.lock();
                residents
                    .iter()
                    .position(|candidate| Arc::ptr_eq(candidate, &fiber))
                    .map(|index| residents.remove(index))
            };
            (removed_group_fiber, removed_resident, removed_allocation)
        };
        // A resident Fiber can own plugin/config values with arbitrary Drop.
        // Move every potentially-last strong reference out of all framework
        // critical sections before destruction.
        drop(removed_group_fiber);
        drop(removed_resident);
        drop(removed_allocation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fiber::Fiber;
    use crate::{Context, Plugin, PreparedPlugin};
    use std::convert::Infallible;
    use std::future::{Future, ready};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct ReentrantDrop {
        registry: Weak<RegistryState>,
        observed_unlocked: Arc<AtomicBool>,
    }

    impl Drop for ReentrantDrop {
        fn drop(&mut self) {
            let unlocked = self.registry.upgrade().is_some_and(|state| {
                state.allocations.try_lock().is_some() && state.residents.try_lock().is_some()
            });
            self.observed_unlocked.store(unlocked, Ordering::SeqCst);
        }
    }

    impl Plugin for ReentrantDrop {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }

        fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            ready(Ok(()))
        }
    }

    fn attach(registry: &Registry, key: PluginKey, fiber: &Arc<Fiber>) {
        registry.attach_fiber(key, fiber.clone());
    }

    #[test]
    fn terminal_close_freezes_detached_residents_and_refuses_late_attach() {
        let registry = Registry::new();
        let key = PluginKey::Typed(TypeId::of::<u32>());
        let existing = Fiber::new("attached before close");
        assert!(registry.attach_fiber(key, existing.clone()));
        let detached = registry.detach_for_removal(key).unwrap().unwrap();
        let frozen = registry.close().unwrap();
        assert_eq!(frozen.len(), 1);
        assert!(Arc::ptr_eq(&frozen[0], &existing));
        // Models an owner paused before native attach after an earlier external
        // cancellation poll returned Pending: close wins the same Registry gate.
        let late = Fiber::new("late attach after close");
        assert!(!registry.attach_fiber(key, late.clone()));
        assert!(late.residency.lock().is_none());
        assert_eq!(registry.admission_count(), 1);
        existing.release_residency();
        assert_eq!(detached.allocation.fiber_count(), 0);
        assert!(registry.close().unwrap().is_empty());
    }

    #[test]
    fn registry_strongly_retains_an_admitted_fiber() {
        let registry = Registry::new();
        let fiber = Fiber::new("resident");
        let weak = Arc::downgrade(&fiber);
        attach(&registry, PluginKey::Anonymous(1), &fiber);

        drop(fiber);

        assert!(weak.upgrade().is_some());
        assert_eq!(registry.resident_fiber_count(), 1);
    }

    #[test]
    fn terminal_unlink_drops_the_last_fiber_outside_registry_locks() {
        let ctx = Context::new();
        let registry = &ctx.root.registry;
        let observed_unlocked = Arc::new(AtomicBool::new(false));
        let prepared = PreparedPlugin::from_input(
            ReentrantDrop {
                registry: Arc::downgrade(&registry.state),
                observed_unlocked: observed_unlocked.clone(),
            },
            (),
        );
        let PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        } = prepared;
        let fiber = Fiber::new(name.clone());
        fiber
            .spawn_state
            .install(
                plugin,
                name,
                inject,
                ctx.clone(),
                PluginKey::Typed(contract),
                ctx.clone(),
            )
            .unwrap();
        registry.attach_fiber(PluginKey::Typed(contract), fiber.clone());

        drop(fiber);
        let resident = registry
            .snapshot_fibers()
            .pop()
            .expect("Fiber remains resident");
        resident.release_residency();
        drop(resident);

        assert!(observed_unlocked.load(Ordering::SeqCst));
        assert_eq!(registry.resident_fiber_count(), 0);
    }

    #[tokio::test]
    async fn typed_and_anonymous_claims_release_and_prune_their_exact_allocation() {
        let registry = Registry::new();
        let cases = [
            PluginKey::Typed(TypeId::of::<u8>()),
            PluginKey::Anonymous(2),
        ];

        for key in cases {
            let fiber = Fiber::new("resident");
            attach(&registry, key, &fiber);
            assert_eq!(registry.resident_fiber_count(), 1);

            fiber.dispose().await;

            assert_eq!(registry.resident_fiber_count(), 0);
            assert!(!registry.state.allocations.lock().contains_key(&key));
        }
    }

    #[test]
    fn detached_members_remain_flat_residents_until_exact_terminal_release() {
        let registry = Registry::new();
        let key = PluginKey::Typed(TypeId::of::<u128>());
        let old = Fiber::new("old");
        attach(&registry, key, &old);

        let detached = registry
            .detach_for_removal(key)
            .unwrap()
            .expect("current typed allocation detaches");
        assert!(registry.state.allocations.lock().get(&key).is_none());
        assert!(
            registry
                .snapshot_fibers()
                .iter()
                .any(|fiber| Arc::ptr_eq(fiber, &old))
        );

        let replacement = Fiber::new("replacement");
        attach(&registry, key, &replacement);
        let residents = registry.snapshot_fibers();
        assert_eq!(residents.len(), 2);
        assert!(residents.iter().any(|fiber| Arc::ptr_eq(fiber, &old)));
        assert!(
            residents
                .iter()
                .any(|fiber| Arc::ptr_eq(fiber, &replacement))
        );

        old.release_residency();
        let residents = registry.snapshot_fibers();
        assert_eq!(residents.len(), 1);
        assert!(Arc::ptr_eq(&residents[0], &replacement));
        assert_eq!(detached.allocation.fiber_count(), 0);

        replacement.release_residency();
        assert!(registry.snapshot_fibers().is_empty());
    }

    #[test]
    fn attach_first_defeats_prune_for_typed_and_anonymous_allocations() {
        let registry = Registry::new();
        for key in [
            PluginKey::Typed(TypeId::of::<u16>()),
            PluginKey::Anonymous(7),
        ] {
            let first = Fiber::new("first");
            attach(&registry, key, &first);
            let allocation = registry.state.allocations.lock().get(&key).unwrap().clone();

            let second = Fiber::new("second");
            attach(&registry, key, &second);
            first.release_residency();

            assert_eq!(allocation.fiber_count(), 1);
            assert!(Arc::ptr_eq(
                registry.state.allocations.lock().get(&key).unwrap(),
                &allocation
            ));
            second.release_residency();
            assert!(!registry.state.allocations.lock().contains_key(&key));
        }
    }

    #[test]
    fn prune_first_forces_a_fresh_typed_or_anonymous_allocation() {
        let registry = Registry::new();
        for key in [
            PluginKey::Typed(TypeId::of::<u32>()),
            PluginKey::Anonymous(8),
        ] {
            let first = Fiber::new("first");
            attach(&registry, key, &first);
            let old = registry.state.allocations.lock().get(&key).unwrap().clone();
            first.release_residency();
            assert!(!registry.state.allocations.lock().contains_key(&key));

            let replacement = Fiber::new("replacement");
            attach(&registry, key, &replacement);
            let fresh = registry.state.allocations.lock().get(&key).unwrap().clone();
            assert!(!Arc::ptr_eq(&old, &fresh));
            replacement.release_residency();
        }
    }

    #[test]
    fn releasing_an_old_claim_cannot_prune_a_typed_or_anonymous_replacement() {
        let registry = Registry::new();
        for key in [
            PluginKey::Typed(TypeId::of::<u64>()),
            PluginKey::Anonymous(9),
        ] {
            let first = Fiber::new("first");
            attach(&registry, key, &first);

            // This is the committed-detach window of Registry removal: the old
            // allocation remains alive while its frozen Fibers are being disposed,
            // but the grouping key already selects a fresh allocation.
            let detached = registry.state.allocations.lock().remove(&key).unwrap();
            let replacement = Fiber::new("replacement");
            attach(&registry, key, &replacement);

            first.release_residency();

            assert_eq!(detached.fiber_count(), 0);
            assert_eq!(registry.resident_fiber_count(), 1);
            let replacement_allocation = replacement
                .residency
                .lock()
                .as_ref()
                .unwrap()
                .allocation
                .upgrade()
                .unwrap();
            assert!(Arc::ptr_eq(
                registry.state.allocations.lock().get(&key).unwrap(),
                &replacement_allocation
            ));
            replacement.release_residency();
            assert_eq!(registry.resident_fiber_count(), 0);
        }
    }
}

#[cfg(test)]
mod caller_driven_owner_tests {
    use super::*;
    use crate::{Context, FiberState, Plugin, PreparedPlugin};
    use futures::FutureExt;
    use std::convert::Infallible;
    use std::panic::AssertUnwindSafe;

    /// One async cleanup gated by the test, so the committed removal owner
    /// reaches its first `Pending` inside the member's terminal drain.
    struct GatedCleanup(Arc<tokio::sync::Notify>);

    impl Plugin for GatedCleanup {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
            let gate = self.0.clone();
            ctx.effect(move || async move { gate.notified().await })
                .unwrap();
            Ok(())
        }
    }

    /// A framework-invariant panic in the typed-removal owner after its
    /// hand-off at the first `Pending` still resumes in the removing caller.
    /// The owner is split at the detach commit exactly as
    /// `Context::remove_plugins` builds it, with a probe panic appended after
    /// the drain.
    #[tokio::test(flavor = "current_thread")]
    async fn removal_owner_panic_after_first_pending_reaches_the_caller() {
        let ctx = Context::new();
        let gate = Arc::new(tokio::sync::Notify::new());
        let handle = ctx
            .spawn(PreparedPlugin::from_input(GatedCleanup(gate.clone()), ()))
            .await
            .unwrap();

        let detached = ctx
            .root
            .registry
            .detach_for_removal(PluginKey::Typed(TypeId::of::<GatedCleanup>()))
            .unwrap()
            .expect("the typed allocation is current");
        let root = ctx.root.clone();
        let mut owner = Box::pin(crate::effect::CallerDriven::new(None, async move {
            detached.dispose_all(&root).await;
            std::panic::panic_any("removal owner probe");
        }));
        assert!(futures::poll!(owner.as_mut()).is_pending());
        gate.notify_one();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            AssertUnwindSafe(owner).catch_unwind(),
        )
        .await
        .expect("the owner reaches its probe");
        let payload = outcome.expect_err("the owner panic reaches the caller");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"removal owner probe"));
        assert_eq!(handle.state(), FiberState::Disposed);
    }
}
