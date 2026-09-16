//! Issue 51 contract evidence for complete ordered Loader outcomes.

use std::cell::RefCell;
use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, ready};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use cordis_core::lifecycle::SpawnError;
use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_loader::outcome::{EntryOutcome, LoaderFailure};
use cordis_loader::plan::{EntryGroup, LoadPlanBuilder, PluginEntry};
use cordis_loader::resolver::PluginRequest;

fn plugin(key: &str) -> PluginEntry {
    PluginEntry {
        key: Some(key.to_owned()),
        name: None,
        config: serde_json::Value::Null,
        disabled: false,
        inject: Vec::new(),
        isolate: Vec::new(),
    }
}

fn disabled_plugin(key: &str) -> PluginEntry {
    let mut entry = plugin(key);
    entry.disabled = true;
    entry
}

struct SuccessPlugin;
impl Plugin for SuccessPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        ready(Ok(()))
    }
}

#[derive(Debug)]
struct ApplyFailure;
impl fmt::Display for ApplyFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("apply failed")
    }
}
impl Error for ApplyFailure {}

struct FailingPlugin;
impl Plugin for FailingPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyFailure;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), ApplyFailure>> + Send {
        ready(Err(ApplyFailure))
    }
}

#[derive(Debug)]
struct ResolveFailure;
impl fmt::Display for ResolveFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("resolver failed")
    }
}
impl Error for ResolveFailure {}

fn resolver_with_log(
    calls: Rc<RefCell<Vec<String>>>,
) -> impl for<'a> Fn(PluginRequest<'a>) -> Result<Option<PreparedPlugin>, ResolveFailure> {
    move |request| {
        let key = request.resolve_key().to_owned();
        calls.borrow_mut().push(key.clone());
        match key.as_str() {
            "missing" => Ok(None),
            "resolver-fail" => Err(ResolveFailure),
            "spawn-fail" => Ok(Some(PreparedPlugin::from_input(FailingPlugin, ()))),
            _ => Ok(Some(PreparedPlugin::from_input(SuccessPlugin, ()))),
        }
    }
}

async fn dispose_spawned(outcome: &cordis_loader::LoadOutcome) {
    for fiber_handle in outcome.fiber_handles() {
        fiber_handle.dispose().await.unwrap();
    }
}

#[tokio::test]
async fn outcomes_are_complete_depth_first_and_pruning_names_the_disabling_plugin() {
    let mut builder = LoadPlanBuilder::new();
    let root = builder
        .add_group(
            None,
            EntryGroup {
                name: "root".into(),
            },
        )
        .unwrap();
    let first = builder.add_plugin(Some(&root), plugin("first")).unwrap();
    let sibling = builder
        .add_group(
            Some(&root),
            EntryGroup {
                name: "sibling".into(),
            },
        )
        .unwrap();
    // Added after its parent's sibling on purpose: execution still follows the
    // frozen parent/child relation depth-first, while sibling declaration order stays stable.
    let nested = builder
        .add_group(
            Some(&first),
            EntryGroup {
                name: "nested".into(),
            },
        )
        .unwrap();
    let disabled = builder
        .add_plugin(Some(&nested), disabled_plugin("disabled"))
        .unwrap();
    let pruned_plugin = builder
        .add_plugin(Some(&disabled), plugin("never-resolve"))
        .unwrap();
    let pruned_group = builder
        .add_group(
            Some(&disabled),
            EntryGroup {
                name: "pruned-group".into(),
            },
        )
        .unwrap();
    let pruned_disabled = builder
        .add_plugin(Some(&pruned_group), disabled_plugin("also-never"))
        .unwrap();
    let after = builder.add_plugin(Some(&sibling), plugin("after")).unwrap();
    let plan = builder.finish().unwrap();

    let calls = Rc::new(RefCell::new(Vec::new()));
    let resolver = resolver_with_log(calls.clone());
    let ctx = Context::new();
    let outcome = plan.load(&ctx, &resolver).await;

    assert_eq!(
        outcome.entries().len(),
        9,
        "one outcome per frozen plan entry"
    );
    let expected = [
        &root,
        &first,
        &nested,
        &disabled,
        &pruned_plugin,
        &pruned_group,
        &pruned_disabled,
        &sibling,
        &after,
    ];
    for (row, id) in outcome.entries().iter().zip(expected) {
        assert_eq!(row.id(), id);
    }
    fn assert_debug<T: fmt::Debug>() {}
    assert_debug::<cordis_loader::LoadOutcome>();

    assert!(matches!(
        outcome.entry(&root),
        Some(EntryOutcome::Group { .. })
    ));
    assert!(
        matches!(outcome.entry(&first), Some(EntryOutcome::Spawned { resolve_key, .. }) if resolve_key == "first")
    );
    assert!(matches!(
        outcome.entry(&nested),
        Some(EntryOutcome::Group { .. })
    ));
    assert!(matches!(
        outcome.entry(&disabled),
        Some(EntryOutcome::Disabled { .. })
    ));
    for id in [&pruned_plugin, &pruned_group, &pruned_disabled] {
        assert!(matches!(
            outcome.entry(id),
            Some(EntryOutcome::Pruned { disabled_ancestor, .. }) if disabled_ancestor == &disabled
        ));
    }
    assert!(matches!(
        outcome.entry(&sibling),
        Some(EntryOutcome::Group { .. })
    ));
    assert!(
        matches!(outcome.entry(&after), Some(EntryOutcome::Spawned { resolve_key, .. }) if resolve_key == "after")
    );
    assert_eq!(
        &*calls.borrow(),
        &["first", "after"],
        "disabled/pruned rows never resolve"
    );
    assert!(outcome.is_ok());
    assert_eq!(outcome.fiber_handles().count(), 2);

    dispose_spawned(&outcome).await;
}

#[tokio::test]
async fn independent_failures_do_not_prune_descendants_or_stop_later_reachable_entries() {
    let mut builder = LoadPlanBuilder::new();
    let failed = builder.add_plugin(None, plugin("resolver-fail")).unwrap();
    let descendant = builder
        .add_plugin(Some(&failed), plugin("descendant-ok"))
        .unwrap();
    let missing = builder.add_plugin(None, plugin("missing")).unwrap();
    let spawn_failed = builder.add_plugin(None, plugin("spawn-fail")).unwrap();
    let later = builder.add_plugin(None, plugin("later-ok")).unwrap();
    let plan = builder.finish().unwrap();

    let calls = Rc::new(RefCell::new(Vec::new()));
    let resolver = resolver_with_log(calls.clone());
    let ctx = Context::new();
    let outcome = plan.load(&ctx, &resolver).await;

    assert_eq!(outcome.entries().len(), 5);
    assert!(matches!(
        outcome.entry(&failed),
        Some(EntryOutcome::Failed {
            resolve_key,
            failure: LoaderFailure::Resolver(_),
            ..
        }) if resolve_key == "resolver-fail"
    ));
    assert!(matches!(
        outcome.entry(&descendant),
        Some(EntryOutcome::Spawned { .. })
    ));
    assert!(matches!(
        outcome.entry(&missing),
        Some(EntryOutcome::Failed { resolve_key, failure: LoaderFailure::UnresolvedKey { key }, .. })
            if resolve_key == "missing" && key == "missing"
    ));
    assert!(matches!(
        outcome.entry(&spawn_failed),
        Some(EntryOutcome::Failed {
            resolve_key,
            failure: LoaderFailure::Spawn(SpawnError::InitialApply(_)),
            ..
        }) if resolve_key == "spawn-fail"
    ));
    assert!(matches!(
        outcome.entry(&later),
        Some(EntryOutcome::Spawned { .. })
    ));
    assert!(
        !outcome.is_ok(),
        "is_ok is false iff at least one Failed row exists"
    );
    assert_eq!(
        outcome.fiber_handles().count(),
        2,
        "successful FiberHandles survive partial failure"
    );
    assert_eq!(
        &*calls.borrow(),
        &[
            "resolver-fail",
            "descendant-ok",
            "missing",
            "spawn-fail",
            "later-ok"
        ]
    );

    dispose_spawned(&outcome).await;
}

struct CaptureContext(Arc<OnceLock<Context>>);
impl Plugin for CaptureContext {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, ctx: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        let _ = self.0.set(ctx);
        ready(Ok(()))
    }
}

#[tokio::test]
async fn inactive_context_reports_each_reachable_plugin_and_leaves_no_runtime_residue() {
    let root = Context::new();
    let captured = Arc::new(OnceLock::new());
    let owner = root
        .spawn(PreparedPlugin::from_input(
            CaptureContext(captured.clone()),
            (),
        ))
        .await
        .unwrap();
    let inactive = captured.get().cloned().unwrap();
    owner.dispose().await.unwrap();
    let before = inactive.runtime_snapshot();
    assert_eq!(before.fibers().len(), 1);
    assert!(before.services().is_empty());

    let mut builder = LoadPlanBuilder::new();
    let group = builder
        .add_group(
            None,
            EntryGroup {
                name: "root".into(),
            },
        )
        .unwrap();
    let first = builder.add_plugin(Some(&group), plugin("first")).unwrap();
    let disabled = builder
        .add_plugin(Some(&group), disabled_plugin("disabled"))
        .unwrap();
    let pruned = builder
        .add_plugin(Some(&disabled), plugin("pruned"))
        .unwrap();
    let second = builder.add_plugin(Some(&group), plugin("second")).unwrap();
    let plan = builder.finish().unwrap();

    let calls = Rc::new(RefCell::new(Vec::new()));
    let resolver = resolver_with_log(calls.clone());
    let outcome = plan.load(&inactive, &resolver).await;

    assert!(matches!(
        outcome.entry(&group),
        Some(EntryOutcome::Group { .. })
    ));
    for id in [&first, &second] {
        assert!(matches!(
            outcome.entry(id),
            Some(EntryOutcome::Failed {
                failure: LoaderFailure::Spawn(SpawnError::InactiveContext),
                ..
            })
        ));
    }
    assert!(matches!(
        outcome.entry(&disabled),
        Some(EntryOutcome::Disabled { .. })
    ));
    assert!(matches!(
        outcome.entry(&pruned),
        Some(EntryOutcome::Pruned { disabled_ancestor, .. }) if disabled_ancestor == &disabled
    ));
    assert_eq!(&*calls.borrow(), &["first", "second"]);
    assert_eq!(outcome.fiber_handles().count(), 0);
    assert!(!outcome.is_ok());

    let after = inactive.runtime_snapshot();
    assert_eq!(
        after.fibers().len(),
        before.fibers().len(),
        "no attempted Fiber remains resident"
    );
    assert_eq!(
        after.services().len(),
        before.services().len(),
        "no publication residue exists"
    );
}

#[tokio::test]
async fn duplicate_resolve_keys_remain_distinct_occurrences_by_entry_order_and_fiber_handle() {
    let mut builder = LoadPlanBuilder::new();
    let mut first_entry = plugin("same");
    first_entry.name = Some("first-display-only".into());
    let first = builder.add_plugin(None, first_entry).unwrap();
    let mut second_entry = plugin("same");
    second_entry.name = Some("second-display-only".into());
    let second = builder.add_plugin(None, second_entry).unwrap();
    let plan = builder.finish().unwrap();

    let calls = Rc::new(RefCell::new(Vec::new()));
    let resolver = resolver_with_log(calls);
    let ctx = Context::new();
    let outcome = plan.load(&ctx, &resolver).await;

    let (first_fiber_handle, second_fiber_handle) =
        match (outcome.entry(&first), outcome.entry(&second)) {
            (
                Some(EntryOutcome::Spawned {
                    resolve_key: first_key,
                    fiber_handle: first_fiber_handle,
                    ..
                }),
                Some(EntryOutcome::Spawned {
                    resolve_key: second_key,
                    fiber_handle: second_fiber_handle,
                    ..
                }),
            ) => {
                assert_eq!(first_key, "same");
                assert_eq!(second_key, "same");
                (first_fiber_handle, second_fiber_handle)
            }
            _ => panic!("expected two spawned occurrences"),
        };
    assert_ne!(first, second);
    assert_ne!(
        first_fiber_handle.id(),
        second_fiber_handle.id(),
        "duplicate resolve keys never collapse Fiber occurrences"
    );
    assert_eq!(outcome.entries()[0].id(), &first);
    assert_eq!(outcome.entries()[1].id(), &second);
    let fiber_handle_ids: Vec<_> = outcome
        .fiber_handles()
        .map(|fiber_handle| fiber_handle.id())
        .collect();
    assert_eq!(
        fiber_handle_ids,
        vec![first_fiber_handle.id(), second_fiber_handle.id()]
    );
    assert!(outcome.is_ok());

    dispose_spawned(&outcome).await;
}

#[tokio::test]
async fn resolver_can_reenter_runtime_observation_before_lifecycle_admission() {
    let ctx = Context::new();
    let mut builder = LoadPlanBuilder::new();
    builder
        .add_plugin(None, plugin("reentrant-resolver"))
        .unwrap();
    let plan = builder.finish().unwrap();
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_in_resolver = observed.clone();
    let resolver_ctx = ctx.clone();
    let resolver =
        move |_request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, Infallible> {
            let snapshot = resolver_ctx.runtime_snapshot();
            assert_eq!(
                snapshot.fibers().len(),
                1,
                "resolver runs before Fiber admission"
            );
            observed_in_resolver.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(PreparedPlugin::from_input(SuccessPlugin, ())))
        };

    let outcome = plan.load(&ctx, &resolver).await;
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
    dispose_spawned(&outcome).await;
}
