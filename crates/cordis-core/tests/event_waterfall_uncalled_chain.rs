//! Issue 196: abort-prone uncalled waterfall chain destruction runs in a child process.

use cordis_core::event::{
    DispatchError, DispatchOutcomeKind, EventOperation, InvocationFailureKind, ListenerOptions,
    mapper_sync, observer_sync,
};
use cordis_core::observation::{ListenerChange, RuntimeObservation};
use cordis_core::{Context, Event, Routing};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::io;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct Flow;
impl Event for Flow {
    const NAME: &'static str = "issue196/uncalled-waterfall-chain";
    type Args = ();
    type Output = ();
}

struct PanicOnDrop(&'static str, Arc<AtomicUsize>);
impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        self.1.fetch_add(1, Ordering::SeqCst);
        panic!("{} destructor", self.0);
    }
}

async fn check_public_dispatch() {
    let ctx = Context::new();
    let (completion_tx, mut completion_rx) = tokio::sync::mpsc::unbounded_channel();
    let (registration_tx, mut registration_rx) = tokio::sync::mpsc::unbounded_channel();
    ctx.observe_runtime(observer_sync(move |_, record: RuntimeObservation| {
        if let RuntimeObservation::ListenerRegistration {
            change: ListenerChange::Registered,
            event: Flow::NAME,
            listener,
            ..
        } = &record
        {
            registration_tx.send(listener.clone()).unwrap();
        }
        if let RuntimeObservation::DispatchCompleted {
            operation: EventOperation::Waterfall,
            event: Flow::NAME,
            outcome,
            ..
        } = record
        {
            completion_tx.send(outcome).unwrap();
        }
        Ok::<(), Infallible>(())
    }))
    .unwrap();

    let drops = Arc::new(AtomicUsize::new(0));
    let later_hits = Arc::new(AtomicUsize::new(0));
    let mut registrations = Vec::new();
    for name in ["B", "C"] {
        let capture = PanicOnDrop(name, drops.clone());
        let hits = later_hits.clone();
        registrations.push(
            ctx.on::<Flow, _>(mapper_sync(move |_, ()| {
                let _ = &capture;
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(())
            }))
            .unwrap(),
        );
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        for _ in 0..2 {
            registration_rx
                .recv()
                .await
                .expect("B/C registration observed");
        }
    })
    .await
    .expect("B/C registration observations were not published");
    let controls = Arc::new(Mutex::new(registrations));
    let removed = controls.clone();
    let retained = drops.clone();
    ctx.on_with::<Flow, _>(
        mapper_sync(move |_, ()| {
            for registration in std::mem::take(&mut *removed.lock()) {
                assert!(registration.remove());
                assert_eq!(retained.load(Ordering::SeqCst), 0);
            }
            Err::<(), _>(io::Error::other("A error"))
        }),
        ListenerOptions::default().prepend(),
    )
    .unwrap();
    let a_id = tokio::time::timeout(Duration::from_secs(2), registration_rx.recv())
        .await
        .expect("A registration observation was not published")
        .expect("A registration observed");

    let tail_hits = Arc::new(AtomicUsize::new(0));
    let hits = tail_hits.clone();
    let Err(DispatchError::Invocation(failure)) = ctx
        .waterfall::<Flow, _, _, Infallible>(Routing::Unscoped, (), move |()| async move {
            hits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
    else {
        panic!("expected the original Mapper failure");
    };
    assert_eq!(failure.kind(), InvocationFailureKind::ReturnedError);
    assert_eq!(failure.registration_id(), Some(&a_id));
    assert!(failure.diagnostic().starts_with("A error; "));
    assert!(failure.diagnostic().contains("B destructor"));
    assert!(failure.diagnostic().contains("C destructor"));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert_eq!(later_hits.load(Ordering::SeqCst), 0);
    assert_eq!(tail_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("waterfall completion was not published"),
        Some(DispatchOutcomeKind::Failed)
    );
}

#[tokio::test]
async fn mapper_error_contains_each_uncalled_listener_destructor() {
    const CHILD: &str = "CORDIS_ISSUE_196_CHILD";
    if std::env::var_os(CHILD).is_some() {
        check_public_dispatch().await;
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mapper_error_contains_each_uncalled_listener_destructor",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("RUST_BACKTRACE", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated dispatch exited {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .take(16)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
