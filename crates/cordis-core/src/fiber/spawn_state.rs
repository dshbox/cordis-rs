//! Spawn-time state for one [`Fiber`].
//!
//! This module owns the optional installation sentinel, its lock, the
//! snapshot rules (ADR 0023), and the pending-change handoff an accepted
//! [`crate::PreparedChange`] update deposits for the next restart pass to
//! install under the inertia-slot claim. Callers ask for the capability
//! they need; neither the stored record nor a lock guard crosses the
//! module seam.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::InjectSpec;
use crate::context::{Context, Root};
use crate::plugin::{PreparedChange, SealedPlugin};
use crate::registry::PluginKey;

use super::Fiber;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpawnStateError;

/// The installed spawn-time record. Its layout is deliberately private:
/// callers receive only purpose-specific owned snapshots.
/// Exact identity of one committed semantically-complete Plugin input.
///
/// `Plugin::Input` intentionally has no `Eq`/`Hash` bound. Cordis therefore
/// tracks the committed input occurrence rather than inspecting or comparing
/// arbitrary user values. The identity is private and never exposed as target
/// mechanism state.
#[derive(Clone)]
pub(super) struct EffectiveApplyInput(Arc<()>);

impl PartialEq for EffectiveApplyInput {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for EffectiveApplyInput {}

impl std::fmt::Debug for EffectiveApplyInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EffectiveApplyInput(..)")
    }
}

struct Stored {
    plugin: Arc<dyn SealedPlugin>,
    name: String,
    inject: InjectSpec,
    scope: Context,
    plugin_key: PluginKey,
    spawning_ctx: Context,
    effective_input: EffectiveApplyInput,
}

/// Everything the settle path needs to invoke one plugin apply.
pub(super) struct ApplySnapshot {
    pub(super) plugin: Arc<dyn SealedPlugin>,
    pub(super) scope: Context,
}

/// The captured creation recipe carried from one ended Fiber to one fresh successor.
pub(super) struct EraRecipe {
    pub(super) plugin: Arc<dyn SealedPlugin>,
    pub(super) name: String,
    pub(super) inject: InjectSpec,
    pub(super) plugin_key: PluginKey,
    pub(super) spawning_ctx: Context,
    pub(super) root: Arc<Root>,
}

/// The deep module around a fiber's spawn-time state.
///
/// Root and isolated test fibers legitimately remain uninstalled. Required
/// internal operations refuse through the private spawn-state invariant; optional
/// projections retain their empty semantics.
pub(crate) struct SpawnState {
    stored: Mutex<Option<Stored>>,
}

impl SpawnState {
    pub(crate) fn new() -> Self {
        Self {
            stored: Mutex::new(None),
        }
    }

    /// Install the record once, after registry attachment and before declared
    /// dependency edges become visible. A repeated install is an invalid
    /// lifecycle transition and never overwrites the first record.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install(
        &self,
        plugin: Arc<dyn SealedPlugin>,
        name: String,
        inject: InjectSpec,
        scope: Context,
        plugin_key: PluginKey,
        spawning_ctx: Context,
    ) -> std::result::Result<(), SpawnStateError> {
        let incoming = Stored {
            plugin,
            name,
            inject,
            scope,
            plugin_key,
            spawning_ctx,
            effective_input: EffectiveApplyInput(Arc::new(())),
        };
        // On rejection `incoming` stays outside the stored slot and drops only
        // after the guard. The existing record is never replaced under lock.
        let rejected = {
            let mut stored = self.stored.lock();
            if stored.is_some() {
                Some(incoming)
            } else {
                *stored = Some(incoming);
                None
            }
        };
        if rejected.is_some() {
            return Err(SpawnStateError);
        }
        Ok(())
    }

    /// The Plugin contract of the installed record for lifecycle preflight.
    /// `None` means no typed creation recipe is installed.
    pub(super) fn contract(&self) -> Option<std::any::TypeId> {
        let stored = self.stored.lock();
        match &stored.as_ref()?.plugin_key {
            PluginKey::Typed(contract) => Some(*contract),
            PluginKey::Anonymous(_) => None,
        }
    }

    /// Commit one admitted candidate while the lifecycle slot is held.
    pub(super) fn commit_change(
        &self,
        change: PreparedChange,
    ) -> std::result::Result<Box<dyn std::any::Any + Send>, SpawnStateError> {
        let plugin = self
            .stored
            .lock()
            .as_ref()
            .ok_or_else(invalid_plugin)?
            .plugin
            .clone();
        let superseded = plugin.swap_input(change).map_err(|_| SpawnStateError)?;
        self.stored
            .lock()
            .as_mut()
            .ok_or_else(invalid_plugin)?
            .effective_input = EffectiveApplyInput(Arc::new(()));
        Ok(superseded)
    }

    /// Derive the fiber-owned context when this is a spawned fiber.
    pub(super) fn context_for(&self, fiber: &Arc<Fiber>) -> Option<Context> {
        self.required_context_for(fiber).ok()
        // The guard drops here, before the caller can run user code on ctx.
    }

    /// Derive the fiber-owned context for an operation that requires an
    /// installed plugin.
    pub(super) fn required_context_for(
        &self,
        fiber: &Arc<Fiber>,
    ) -> std::result::Result<Context, SpawnStateError> {
        let stored = self.stored.lock();
        let Some(state) = stored.as_ref() else {
            return Err(invalid_plugin());
        };
        Ok(state.scope.with_fiber(fiber.clone()))
    }

    /// The owning root when installed. The owned clone lets dependency and
    /// registry work happen after the state lock is released.
    pub(super) fn root(&self) -> Option<Arc<Root>> {
        self.required_root().ok()
    }

    /// The owning root for an operation that requires an installed plugin.
    pub(super) fn required_root(&self) -> std::result::Result<Arc<Root>, SpawnStateError> {
        let stored = self.stored.lock();
        let Some(state) = stored.as_ref() else {
            return Err(invalid_plugin());
        };
        Ok(state.scope.root.clone())
    }

    /// Identity of the currently committed apply-relevant Plugin input.
    /// It changes only when an accepted update commits a new input value.
    pub(super) fn effective_input(
        &self,
    ) -> std::result::Result<EffectiveApplyInput, SpawnStateError> {
        let stored = self.stored.lock();
        let Some(state) = stored.as_ref() else {
            return Err(invalid_plugin());
        };
        Ok(state.effective_input.clone())
    }

    /// Snapshot one apply invocation, or reject a fiber that was never
    /// installed as a plugin.
    pub(super) fn apply_snapshot(&self) -> std::result::Result<ApplySnapshot, SpawnStateError> {
        let stored = self.stored.lock();
        let Some(state) = stored.as_ref() else {
            return Err(invalid_plugin());
        };
        Ok(ApplySnapshot {
            plugin: state.plugin.clone(),
            scope: state.scope.clone(),
        })
    }

    /// Snapshot the creation recipe needed by one era replacement.
    pub(super) fn era_recipe(&self) -> std::result::Result<EraRecipe, SpawnStateError> {
        let stored = self.stored.lock();
        let Some(state) = stored.as_ref() else {
            return Err(invalid_plugin());
        };
        Ok(EraRecipe {
            plugin: state.plugin.clone(),
            name: state.name.clone(),
            inject: state.inject.clone(),
            plugin_key: state.plugin_key,
            spawning_ctx: state.spawning_ctx.clone(),
            root: state.scope.root.clone(),
        })
    }
}

fn invalid_plugin() -> SpawnStateError {
    SpawnStateError
}

#[cfg(test)]
mod tests {
    use super::{SpawnState, SpawnStateError};
    use crate::context::Context;
    use crate::fiber::Fiber;
    use crate::registry::PluginKey;
    use crate::{Plugin, PreparedPlugin};
    use std::convert::Infallible;
    use std::future::{Future, ready};
    use std::sync::Arc;

    struct Noop;

    impl Plugin for Noop {
        type Config = u8;
        type Input = u8;
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, config: u8) -> std::result::Result<u8, Infallible> {
            Ok(config)
        }

        fn apply(
            &self,
            _ctx: Context,
            _input: &u8,
        ) -> impl Future<Output = std::result::Result<(), Infallible>> + Send {
            ready(Ok(()))
        }
    }

    fn install(state: &SpawnState, ctx: &Context, value: u8) -> PluginKey {
        let PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        } = PreparedPlugin::from_input(Noop, value);
        let plugin_key = PluginKey::Typed(contract);
        state
            .install(plugin, name, inject, ctx.clone(), plugin_key, ctx.clone())
            .unwrap();
        plugin_key
    }

    #[test]
    fn absence_is_optional_for_projections_and_typed_for_required_operations() {
        let state = SpawnState::new();
        let fiber = Fiber::new("test");

        assert!(state.context_for(&fiber).is_none());
        assert!(state.root().is_none());
        assert!(matches!(state.apply_snapshot(), Err(SpawnStateError)));
        assert!(matches!(state.era_recipe(), Err(SpawnStateError)));
        assert_eq!(state.contract(), None);
    }

    #[tokio::test]
    async fn a_deposited_change_installs_into_the_sealed_state() {
        struct Recorder {
            seen: Arc<parking_lot::Mutex<Vec<u8>>>,
        }

        impl Plugin for Recorder {
            type Config = u8;
            type Input = u8;
            type PrepareError = Infallible;
            type ApplyError = Infallible;

            fn prepare(&self, config: u8) -> std::result::Result<u8, Infallible> {
                Ok(config)
            }

            fn apply(
                &self,
                _ctx: Context,
                input: &u8,
            ) -> impl Future<Output = std::result::Result<(), Infallible>> + Send {
                self.seen.lock().push(*input);
                ready(Ok(()))
            }
        }

        let ctx = Context::new();
        let state = SpawnState::new();
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        } = PreparedPlugin::from_input(Recorder { seen: seen.clone() }, 3);
        state
            .install(
                plugin,
                name,
                inject,
                ctx.clone(),
                PluginKey::Typed(contract),
                ctx.clone(),
            )
            .unwrap();

        state
            .commit_change(crate::PreparedChange::from_input::<Recorder>(9))
            .unwrap();

        let snapshot = state.apply_snapshot().unwrap();
        snapshot
            .plugin
            .apply_boxed(snapshot.scope.clone())
            .await
            .unwrap();
        assert_eq!(
            *seen.lock(),
            vec![9],
            "the installed candidate, not the spawn-time value, reaches apply"
        );
    }

    #[test]
    fn install_is_once_and_snapshots_stay_narrow() {
        let ctx = Context::new();
        let state = SpawnState::new();
        let fiber = Fiber::new("test");

        let plugin_key = install(&state, &ctx, 3);

        assert!(state.context_for(&fiber).is_some());
        assert!(Arc::ptr_eq(&state.root().unwrap(), &ctx.root));
        assert!(state.apply_snapshot().is_ok());
        assert_eq!(state.contract(), Some(std::any::TypeId::of::<Noop>()));
        let recipe = state.era_recipe().unwrap();
        assert_eq!(recipe.plugin_key, plugin_key);
        assert!(Arc::ptr_eq(&recipe.root, &ctx.root));

        let PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        } = PreparedPlugin::from_input(Noop, 4);
        let err = state
            .install(
                plugin,
                name,
                inject,
                ctx.clone(),
                PluginKey::Typed(contract),
                ctx.clone(),
            )
            .unwrap_err();
        assert_eq!(err, SpawnStateError);
    }
}
