#![cfg(feature = "macros")]

//! Regression tests for bugs found across two audit rounds.
//!
//! ## Audit 1 (P0 / P1)
//! - P0 #1: terminate() passes ExitCode::Terminated
//! - P0 #2: RootContext::terminate() handles Terminate without panic
//! - P0 #3: ask().timeout().await compiles and works
//! - P0 #4: panic_msg handles non-string panic payloads
//! - P1 #5: sig_rx.recv().unwrap() — not a real bug
//! - P1 #6: parent_hdl ChildDropped send — not a real bug
//! - P1 #7: root escalation handling
//!
//! ## Audit 2
//! - Bug #1: Init-failure deadlocks parent (wait_signal drops ack sender)
//! - Bug #2: RootContext::terminate() returns before children finish
//! - Bug #3: Lazy cleanup of dead WeakActorHdl in child_hdls

use std::sync::{
    Arc, Once,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::actor;
use tokio::time::sleep;

static INIT: Once = Once::new();

fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter("trace")
            .try_init()
            .ok();
    });
}

// ============================================================
// Audit 1 — actors & messages
// ============================================================

#[derive(Debug, Clone)]
struct ExitCodeTracker {
    was_terminated: Arc<AtomicBool>,
    was_dropped: Arc<AtomicBool>,
}

impl ActorArgs for ExitCodeTracker {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanicMsg;

#[actor("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")]
impl Actor for ExitCodeTracker {
    const _: () = {
        async |_: PanicMsg| {
            panic!("trigger escalation");
        };
    };

    async fn on_exit(&mut self, code: ExitCode) {
        match code {
            ExitCode::Terminated => self.was_terminated.store(true, Ordering::SeqCst),
            ExitCode::Dropped => self.was_dropped.store(true, Ordering::SeqCst),
        }
    }
}

#[derive(Debug, Clone)]
struct SimpleActor;

impl ActorArgs for SimpleActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Noop;

#[actor("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")]
impl Actor for SimpleActor {
    const _: () = {
        async |_: Noop| {};
    };
}

#[derive(Debug, Clone)]
struct TimeoutActor;

impl ActorArgs for TimeoutActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskWithTimeout;

#[actor("cccccccc-cccc-cccc-cccc-cccccccccccc")]
impl Actor for TimeoutActor {
    const _: () = {
        async |_: AskWithTimeout| -> u64 { 42 };
    };
}

#[derive(Debug, Clone)]
struct NonStringPanicActor {
    exited: Arc<AtomicBool>,
}

impl ActorArgs for NonStringPanicActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanicWithInt;

#[actor("dddddddd-dddd-dddd-dddd-dddddddddddd")]
impl Actor for NonStringPanicActor {
    const _: () = {
        async |_: PanicWithInt| {
            std::panic::panic_any(42u32);
        };
    };

    async fn on_exit(&mut self, _code: ExitCode) {
        self.exited.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone)]
struct ParentActor;

impl ActorArgs for ParentActor {
    type Actor = Self;
    async fn initialize(ctx: Context<Self>, args: &Self) -> Self {
        let _child = ctx.spawn(SimpleActor);
        args.clone()
    }
}

#[actor("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee")]
impl Actor for ParentActor {
    const _: () = async |_: Noop| {};
}

// ============================================================
// Audit 1 — tests
// ============================================================

/// P0 #1: terminate() should pass ExitCode::Terminated
#[tokio::test]
async fn p0_1_terminate_should_pass_exit_code_terminated() {
    init_tracing();
    let ctx = RootContext::default();
    let was_terminated = Arc::new(AtomicBool::new(false));
    let was_dropped = Arc::new(AtomicBool::new(false));

    let actor = ctx.spawn(ExitCodeTracker {
        was_terminated: was_terminated.clone(),
        was_dropped: was_dropped.clone(),
    });

    let _ = actor.tell(PanicMsg);
    sleep(Duration::from_millis(300)).await;

    assert!(
        was_terminated.load(Ordering::SeqCst),
        "on_exit should receive ExitCode::Terminated"
    );
    assert!(
        !was_dropped.load(Ordering::SeqCst),
        "on_exit should NOT receive ExitCode::Dropped"
    );
}

/// P0 #2: RootContext should handle Terminate signal without panic
#[tokio::test]
async fn p0_2_root_context_terminate_should_not_panic() {
    init_tracing();
    let ctx = RootContext::default();
    let _actor = ctx.spawn(SimpleActor);

    ctx.terminate().await;
}

/// P0 #3: ask().timeout().await should compile and work
#[tokio::test]
async fn p0_3_ask_timeout_should_compile_and_work() {
    init_tracing();
    let ctx = RootContext::default();
    let actor = ctx.spawn(TimeoutActor);

    let result = actor
        .ask(AskWithTimeout)
        .timeout(Duration::from_secs(1))
        .await;
    assert_eq!(result.unwrap(), 42);
}

/// P0 #4: panic_msg should handle non-string panic payloads
#[tokio::test]
async fn p0_4_panic_msg_should_handle_non_string_payloads() {
    init_tracing();
    let ctx = RootContext::default();
    let exited = Arc::new(AtomicBool::new(false));

    let actor = ctx.spawn(NonStringPanicActor {
        exited: exited.clone(),
    });

    let _ = actor.tell(PanicWithInt);
    sleep(Duration::from_millis(300)).await;

    assert!(
        exited.load(Ordering::SeqCst),
        "on_exit should fire even with non-string panic payload"
    );
}

/// P1 #5: sig_rx.recv().unwrap() — NOT a real bug
#[tokio::test]
async fn p1_5_sig_rx_unwrap_cannot_be_triggered() {
    init_tracing();
    let ctx = RootContext::default();
    let _actor = ctx.spawn(SimpleActor);

    ctx.terminate().await;
}

/// P1 #6: parent_hdl ChildDropped send — NOT a real bug
#[tokio::test]
async fn p1_6_drop_lifecycle_child_dropped_send_works() {
    init_tracing();
    let ctx = RootContext::default();
    let parent = ctx.spawn(ParentActor);

    drop(parent);
    sleep(Duration::from_millis(200)).await;
}

/// P1 #7: root escalation unwrap — already fixed in P0 #2
#[tokio::test]
async fn p1_7_root_escalation_handling_works() {
    init_tracing();
    let ctx = RootContext::default();
    let actor = ctx.spawn(ExitCodeTracker {
        was_terminated: Arc::new(AtomicBool::new(false)),
        was_dropped: Arc::new(AtomicBool::new(false)),
    });

    let _ = actor.tell(PanicMsg);
    sleep(Duration::from_millis(300)).await;

    ctx.terminate().await;
}

// ============================================================
// Audit 2 — actors & messages
// ============================================================

#[derive(Debug, Clone)]
pub struct ParentOfFailChild {
    child_spawned: Arc<AtomicBool>,
}

impl ActorArgs for ParentOfFailChild {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnFailChild;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping;

#[actor("11111111-1111-1111-1111-111111111111")]
impl Actor for ParentOfFailChild {
    const _: () = {
        async |SpawnFailChild: SpawnFailChild| {
            self.child_spawned.store(true, Ordering::SeqCst);
            let _child = ctx.spawn(FailingChild);
        };

        async |Ping: Ping| -> u64 { 42 };
    };
}

/// An actor whose initialization always panics.
#[derive(Debug, Clone)]
pub struct FailingChild;

impl ActorArgs for FailingChild {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, _args: &Self) -> Self {
        panic!("intentional init failure");
    }
}

#[actor("22222222-2222-2222-2222-222222222222")]
impl Actor for FailingChild {}

#[derive(Debug, Clone)]
pub struct SlowExitActor {
    exited: Arc<AtomicBool>,
}

impl ActorArgs for SlowExitActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Noop2;

#[actor("33333333-3333-3333-3333-333333333333")]
impl Actor for SlowExitActor {
    const _: () = {
        async |Noop2: Noop2| {};
    };

    async fn on_exit(&mut self, _code: ExitCode) {
        sleep(Duration::from_millis(500)).await;
        self.exited.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone)]
pub struct DropTracker {
    flag: Arc<AtomicBool>,
}

impl ActorArgs for DropTracker {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Noop3;

#[actor("66666666-6666-6666-6666-666666666666")]
impl Actor for DropTracker {
    const _: () = {
        async |Noop3: Noop3| {};
    };

    async fn on_exit(&mut self, _code: ExitCode) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

// ============================================================
// Audit 2 — tests
// ============================================================

/// Bug #1: Init-failure deadlocks parent.
///
/// When a child panics during `initialize()`, it escalates to the parent.
/// Default supervision sends `Signal::Terminate` back but `wait_signal()`
/// only handled `Restart` — other signals were dropped without sending the
/// oneshot ack, hanging the parent forever in `supervise()`.
#[tokio::test]
async fn bug1_init_failure_deadlocks_parent() {
    init_tracing();
    let ctx = RootContext::default();

    let child_spawned = Arc::new(AtomicBool::new(false));

    let parent = ctx.spawn(ParentOfFailChild {
        child_spawned: child_spawned.clone(),
    });

    let _ = parent.tell(SpawnFailChild);
    sleep(Duration::from_millis(500)).await;

    assert!(
        child_spawned.load(Ordering::SeqCst),
        "parent should have spawned the child"
    );

    let result = parent.ask(Ping).timeout(Duration::from_secs(2)).await;

    assert!(
        result.is_ok(),
        "parent should still be responsive after child init failure, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), 42);
}

/// Bug #2: `RootContext::terminate()` returns before children finish.
///
/// Root supervision sent `Terminate(None)` fire-and-forget, then immediately
/// ack'd the caller. Fixed by awaiting each child's ack sequentially.
#[tokio::test]
async fn bug2_root_terminate_does_not_wait_for_children() {
    init_tracing();
    let ctx = RootContext::default();

    let exited = Arc::new(AtomicBool::new(false));

    let actor = ctx.spawn(SlowExitActor {
        exited: exited.clone(),
    });

    let _ = actor.tell(Noop2);
    sleep(Duration::from_millis(100)).await;

    ctx.terminate().await;

    assert!(
        exited.load(Ordering::SeqCst),
        "on_exit() should have completed before terminate().await returns"
    );
}

/// Bug #3: Terminated actors' parent never cleaned `child_hdls`.
///
/// Dropped actors sent `ChildDropped`; terminated actors did not.
/// Fixed with lazy pruning of dead `WeakActorHdl` entries during iteration.
#[tokio::test]
async fn bug3_lazy_child_hdls_cleanup() {
    init_tracing();
    let ctx = RootContext::default();

    let exited1 = Arc::new(AtomicBool::new(false));
    let exited2 = Arc::new(AtomicBool::new(false));

    let actor1 = ctx.spawn(DropTracker {
        flag: exited1.clone(),
    });
    let actor2 = ctx.spawn(DropTracker {
        flag: exited2.clone(),
    });

    let _ = actor1.tell(Noop3);
    let _ = actor2.tell(Noop3);
    sleep(Duration::from_millis(100)).await;

    drop(actor1);
    sleep(Duration::from_millis(200)).await;
    assert!(
        exited1.load(Ordering::SeqCst),
        "dropped actor should have called on_exit"
    );

    ctx.terminate().await;

    assert!(
        exited2.load(Ordering::SeqCst),
        "terminated actor should have called on_exit via root terminate"
    );
}
