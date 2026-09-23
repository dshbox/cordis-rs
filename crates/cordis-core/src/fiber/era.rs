//! Identity-breaking replacement of one live Fiber with one fresh era.
//!
//! Era replacement is a lifecycle transaction, not a same-Fiber settle pass:
//! preflight is effect-free, one live source is claimed, the old terminal
//! barrier completes, one fresh successor is created from the captured creation
//! recipe plus a `PreparedChange`, affected dependents converge to their current
//! targets, and only then may the fresh `FiberHandle` be handed off.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::context::{RealmKey, Root};
use crate::plugin::{PreparedChange, PreparedPlugin};

use super::spawn::{SpawnError, spawn_prepared_era_successor};
use super::{EraSwapError, EraSwapFailure, Fiber, FiberHandle, LifecycleOperation};

struct EraHandoff {
    fiber_handle: FiberHandle,
    guard: EraHandoffGuard,
}

impl EraHandoff {
    fn accept(self) -> FiberHandle {
        self.guard.disarm();
        self.fiber_handle
    }
}

struct EraHandoffGuard {
    root: Arc<Root>,
    old_publication_edges: Vec<(String, RealmKey)>,
    source: Arc<Fiber>,
    successor: Option<Arc<Fiber>>,
    attribution: super::settle_ctx::SettleCtx,
}

impl EraHandoffGuard {
    fn new(
        root: Arc<Root>,
        edges: Vec<(String, RealmKey)>,
        source: Arc<Fiber>,
        successor: Arc<Fiber>,
    ) -> Self {
        Self {
            root,
            old_publication_edges: edges,
            source,
            successor: Some(successor),
            attribution: super::settle_ctx::capture_attribution(),
        }
    }

    fn disarm(mut self) {
        self.successor.take();
    }
}

impl Drop for EraHandoffGuard {
    fn drop(&mut self) {
        let Some(successor) = self.successor.take() else {
            return;
        };
        let root = self.root.clone();
        let edges = self.old_publication_edges.clone();
        let source = self.source.clone();
        let attribution = self.attribution.clone();
        crate::effect::detach(super::settle_ctx::with_attribution(
            attribution,
            async move {
                cleanup_undelivered_successor(&root, &edges, &source, FiberHandle::new(successor))
                    .await;
            },
        ));
    }
}

impl FiberHandle {
    /// Replace this live Fiber with one freshly created successor era.
    ///
    /// Exact-allocation recursion is refused before liveness, compatibility,
    /// source claim, or successor allocation. Exactly one racing replacement may
    /// claim a live source; a loser, or a replacement
    /// racing a committed ordinary disposal, returns [`EraSwapError::Closed`]
    /// before successor allocation. Once the source claim commits, framework
    /// ownership completes the old terminal barrier, successor attempt, final
    /// affected-dependent convergence, and handoff-or-cleanup independently of
    /// caller polling.
    ///
    /// The successor replays only the captured Plugin behavior, spawn origin,
    /// and private grouping equivalence with the supplied prepared input replacement.
    /// It receives a fresh [`FiberId`](super::FiberId), freshly resolved exact
    /// dependency edges, a fresh sibling-era Event Scope beneath the captured
    /// origin, fresh generations, and fresh publication occurrences. No old
    /// registration, effect, publication, Scope, or spawned Fiber is migrated.
    pub async fn era_swap(
        &self,
        change: PreparedChange,
    ) -> std::result::Result<FiberHandle, EraSwapError> {
        super::settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::EraSwap)
            .map_err(EraSwapError::Recursion)?;
        if !self.fiber.is_alive() || self.fiber.disposing.load(Ordering::SeqCst) {
            return Err(EraSwapError::Closed);
        }
        if self.fiber.spawn_state.contract() != Some(change.contract()) {
            return Err(EraSwapError::PluginContractMismatch);
        }

        let recipe = self
            .fiber
            .spawn_state
            .era_recipe()
            .map_err(|_| EraSwapError::Closed)?;
        let old_publication_edges = recipe.root.services.slots_owned_by(&self.fiber);
        let entry_dependents =
            capture_dependents(&recipe.root, &old_publication_edges, &[&self.fiber]);

        for dependent in &entry_dependents {
            super::settle_ctx::refuse_recursion(dependent, LifecycleOperation::EraSwap)
                .map_err(EraSwapError::Recursion)?;
        }
        // Fresh-query dependents are not knowable yet. Preserve the current
        // exact-allocation attribution across the postclaim detach so their
        // later `ready` backstop can make the same self-wait decision.
        let caller_attribution = super::settle_ctx::capture_attribution();

        // Waiting for the lifecycle slot remains precommit: cancellation here
        // changes nothing. Under the slot, `disposing` is the one live-source
        // arbitration shared with ordinary terminal disposal.
        self.fiber.slot.claim().await;
        if !self.fiber.is_alive() || self.fiber.claim_terminal() {
            self.fiber.slot.abandon();
            return Err(EraSwapError::Closed);
        }

        let source = self.fiber.clone();
        let completion_root = recipe.root.clone();
        let completion_edges = old_publication_edges.clone();
        let completion_source = source.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        crate::effect::detach(super::settle_ctx::with_attribution(
            caller_attribution,
            async move {
                let result = run_committed_replacement(
                    source,
                    recipe,
                    change,
                    old_publication_edges,
                    entry_dependents,
                )
                .await;

                match result {
                    Err(error) => {
                        let _ = send.send(Err(error));
                    }
                    Ok(successor) => {
                        let guard = EraHandoffGuard::new(
                            completion_root.clone(),
                            completion_edges.clone(),
                            completion_source.clone(),
                            successor.fiber.clone(),
                        );
                        let offer = EraHandoff {
                            fiber_handle: successor,
                            guard,
                        };
                        let _ = send.send(Ok(offer));
                    }
                }
            },
        ));

        let completion = receive
            .await
            .expect("era replacement owner retains its completion sender");
        match completion {
            Err(error) => Err(error),
            Ok(handoff) => Ok(handoff.accept()),
        }
    }
}

async fn cleanup_undelivered_successor(
    root: &Arc<Root>,
    old_publication_edges: &[(String, RealmKey)],
    source: &Arc<Fiber>,
    successor: FiberHandle,
) {
    let successor_edges = root.services.slots_owned_by(&successor.fiber);
    successor.fiber.dispose().await;
    let mut final_edges = old_publication_edges.to_vec();
    final_edges.extend(successor_edges);
    converge(capture_dependents(
        root,
        &final_edges,
        &[source.as_ref(), successor.fiber.as_ref()],
    ))
    .await;
}

async fn run_committed_replacement(
    source: Arc<Fiber>,
    recipe: super::spawn_state::EraRecipe,
    change: PreparedChange,
    old_publication_edges: Vec<(String, RealmKey)>,
    entry_dependents: Vec<Arc<Fiber>>,
) -> std::result::Result<FiberHandle, EraSwapError> {
    // The source claim already owns the lifecycle slot and closed every
    // generation gate through `disposing`. The Fiber lifecycle owner completes
    // the same full terminal barrier used by ordinary disposal.
    source.complete_claimed_dispose().await;

    // Observe the entry set after old publication withdrawal and before birth.
    converge(entry_dependents).await;

    let super::spawn_state::EraRecipe {
        plugin,
        name,
        inject,
        plugin_key,
        spawning_ctx,
        root,
    } = recipe;
    let contract = match plugin_key {
        crate::registry::PluginKey::Typed(contract) => contract,
        crate::registry::PluginKey::Anonymous(_) => {
            unreachable!("era recipes are installed only for typed Plugin Fibers")
        }
    };

    let plugin = plugin.into_successor(change).unwrap_or_else(|_| {
        panic!("the claimed source retains one compatible successor candidate")
    });

    let successor = match spawn_prepared_era_successor(
        &spawning_ctx,
        PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        },
    )
    .await
    {
        Ok(successor) => successor,
        Err(error) => {
            // Creation failures are complete before successor creation returns:
            // pre-allocation refusals allocated nothing, while apply failure or
            // framework invalidation fully disposed and unlinked the attempted
            // Fiber. The era transaction still owns one final fresh dependent
            // query so an Incomplete result cannot outrun target changes that
            // landed while successor creation was in flight.
            converge_final(&root, &old_publication_edges, &source, None).await;
            return Err(EraSwapError::Incomplete(map_spawn_failure(error)));
        }
    };

    // Ordinary creation hands off only live quiescent Active/stable Pending.
    // Re-query affected dependents *after* successor visibility so arrivals
    // during the swap are included and all waits target current semantics.
    converge_final(
        &root,
        &old_publication_edges,
        &source,
        Some(&successor.fiber),
    )
    .await;
    Ok(successor)
}

fn map_spawn_failure(error: SpawnError) -> EraSwapFailure {
    match error {
        SpawnError::InitialApply(failure) => EraSwapFailure::SuccessorApply(failure),
        SpawnError::Interrupted => EraSwapFailure::SuccessorLost,
        SpawnError::InactiveContext => {
            unreachable!("era successor replay does not gate on spawn-origin liveness")
        }
    }
}

async fn converge_final(
    root: &Arc<Root>,
    old_publication_edges: &[(String, RealmKey)],
    source: &Arc<Fiber>,
    successor: Option<&Arc<Fiber>>,
) {
    let mut edges = old_publication_edges.to_vec();
    if let Some(successor) = successor {
        edges.extend(root.services.slots_owned_by(successor));
    }
    let mut skip = vec![source.as_ref()];
    if let Some(successor) = successor {
        skip.push(successor.as_ref());
    }
    converge(capture_dependents(root, &edges, &skip)).await;
}

async fn converge(fibers: Vec<Arc<Fiber>>) {
    for fiber in fibers {
        let _ = FiberHandle::new(fiber).ready().await;
    }
}

fn capture_dependents(
    root: &Arc<Root>,
    publication_edges: &[(String, RealmKey)],
    skip: &[&Fiber],
) -> Vec<Arc<Fiber>> {
    let mut captured = root.dependents_for_edges(publication_edges);
    captured.retain(|fiber| !skip.iter().any(|skip| std::ptr::eq(*skip, fiber.as_ref())));
    captured
}

#[cfg(test)]
mod tests {
    use super::{EraHandoff, EraHandoffGuard, map_spawn_failure};
    use crate::fiber::{EraSwapError, EraSwapFailure, FiberState, SpawnError};
    use crate::{Context, Plugin, PreparedChange, PreparedPlugin};
    use std::convert::Infallible;

    struct AllocationProbe;
    impl Plugin for AllocationProbe {
        type Config = u8;
        type Input = u8;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, value: u8) -> Result<u8, Infallible> {
            Ok(value)
        }
        async fn apply(&self, _: Context, _: &u8) -> Result<(), Infallible> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn racing_swaps_admit_exactly_one_successor_fiber() {
        let root = Context::new();
        let source = root
            .spawn(PreparedPlugin::from_input(AllocationProbe, 1))
            .await
            .unwrap();
        let before = root.root.registry.admission_count();
        let (left, right) = tokio::join!(
            source.era_swap(PreparedChange::from_input::<AllocationProbe>(2)),
            source.era_swap(PreparedChange::from_input::<AllocationProbe>(3)),
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let loser = if let Err(error) = &left {
            error
        } else {
            right.as_ref().unwrap_err()
        };
        assert!(matches!(loser, EraSwapError::Closed));
        assert_eq!(root.root.registry.admission_count(), before + 1);
        let winner = left.ok().or_else(|| right.ok()).unwrap();
        winner.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn unconsumed_success_offer_cleans_the_offered_successor() {
        let root = Context::new();
        let source = root
            .spawn(PreparedPlugin::from_input(AllocationProbe, 1))
            .await
            .unwrap();
        let successor = root
            .spawn(PreparedPlugin::from_input(AllocationProbe, 2))
            .await
            .unwrap();
        let guard = EraHandoffGuard::new(
            root.root.clone(),
            Vec::new(),
            source.fiber.clone(),
            successor.fiber.clone(),
        );
        let offer = EraHandoff {
            fiber_handle: successor.clone(),
            guard,
        };
        let (send, receive) = tokio::sync::oneshot::channel::<Result<EraHandoff, EraSwapError>>();
        assert!(send.send(Ok(offer)).is_ok());
        drop(receive);
        successor
            .wait_state(FiberState::Disposed, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        source.dispose().await.unwrap();
    }

    #[test]
    fn only_framework_interruption_maps_to_successor_lost_at_the_creation_seam() {
        assert!(matches!(
            map_spawn_failure(SpawnError::Interrupted),
            EraSwapFailure::SuccessorLost
        ));
    }
}
