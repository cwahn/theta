#![cfg(feature = "macros")]

use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Once,
};
use std::time::Duration;
use theta::prelude::*;
use theta_macros::{ActorArgs, actor};
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
// P0 #1: terminate() should pass ExitCode::Terminated
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
struct PanicMsg;

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

    // Panic triggers escalation → root sends Terminate
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

// ============================================================
// P0 #2: RootContext should handle Terminate signal without panic
// ============================================================

#[derive(Debug, Clone)]
struct SimpleActor;

impl ActorArgs for SimpleActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Noop;

#[actor("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")]
impl Actor for SimpleActor {
    const _: () = {
        async |_: Noop| {};
    };
}

#[tokio::test]
async fn p0_2_root_context_terminate_should_not_panic() {
    init_tracing();
    let ctx = RootContext::default();
    let _actor = ctx.spawn(SimpleActor);

    // This sends Terminate to the root's signal channel.
    // On main (unfixed), this hits unreachable!() and panics the supervision task.
    ctx.terminate().await;
    // If we get here, the root handled Terminate correctly
}

// ============================================================
// P0 #3: ask().timeout().await should compile and work
// ============================================================

#[derive(Debug, Clone)]
struct TimeoutActor;

impl ActorArgs for TimeoutActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AskWithTimeout;

#[actor("cccccccc-cccc-cccc-cccc-cccccccccccc")]
impl Actor for TimeoutActor {
    const _: () = {
        async |_: AskWithTimeout| -> u64 {
            42
        };
    };
}

#[tokio::test]
async fn p0_3_ask_timeout_should_compile_and_work() {
    init_tracing();
    let ctx = RootContext::default();
    let actor = ctx.spawn(TimeoutActor);

    // On main (unfixed), this doesn't compile (E0277) because the original
    // Deadline IntoFuture bound was Result<M, RequestError<M>> (same M),
    // but MsgRequest returns Result<u64, RequestError<MsgPack<TimeoutActor>>>.
    let result = actor.ask(AskWithTimeout).timeout(Duration::from_secs(1)).await;
    assert_eq!(result.unwrap(), 42);
}

// ============================================================
// P0 #4: panic_msg should handle non-string panic payloads
// ============================================================

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
struct PanicWithInt;

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

#[tokio::test]
async fn p0_4_panic_msg_should_handle_non_string_payloads() {
    init_tracing();
    let ctx = RootContext::default();
    let exited = Arc::new(AtomicBool::new(false));

    let actor = ctx.spawn(NonStringPanicActor {
        exited: exited.clone(),
    });

    // panic_any(42u32) produces a non-string payload.
    // On main (unfixed), panic_msg() hits unreachable!(),
    // killing the supervision task silently (on_exit never fires).
    let _ = actor.tell(PanicWithInt);
    sleep(Duration::from_millis(300)).await;

    assert!(
        exited.load(Ordering::SeqCst),
        "on_exit should fire even with non-string panic payload"
    );
}

// ============================================================
// P1 #5: sig_rx.recv().unwrap() — NOT a real bug
// ============================================================

#[tokio::test]
async fn p1_5_sig_rx_unwrap_cannot_be_triggered() {
    init_tracing();
    let ctx = RootContext::default();
    let _actor = ctx.spawn(SimpleActor);

    // Terminate cleanly — if sig_rx.recv().unwrap() were a problem,
    // the actor would panic during shutdown. It doesn't because
    // the actor holds its own this_hdl (SigTx), keeping sig_rx open.
    ctx.terminate().await;
}

// ============================================================
// P1 #6: parent_hdl ChildDropped send — NOT a real bug
// ============================================================

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
    const _: () = {
        async |_: Noop| {};
    };
}

#[tokio::test]
async fn p1_6_drop_lifecycle_child_dropped_send_works() {
    init_tracing();
    let ctx = RootContext::default();
    let parent = ctx.spawn(ParentActor);

    // Drop the only external reference — triggers parent drop,
    // which triggers child drop. If parent_hdl.raw_send(ChildDropped)
    // could fail, the child's Drop impl would panic.
    drop(parent);
    sleep(Duration::from_millis(200)).await;
    // No panic = invariant holds
}

// ============================================================
// P1 #7: root escalation unwrap — already fixed in P0#2
// ============================================================

#[tokio::test]
async fn p1_7_root_escalation_handling_works() {
    init_tracing();
    let ctx = RootContext::default();
    let actor = ctx.spawn(ExitCodeTracker {
        was_terminated: Arc::new(AtomicBool::new(false)),
        was_dropped: Arc::new(AtomicBool::new(false)),
    });

    // Trigger escalation to root — root should handle it without panicking
    let _ = actor.tell(PanicMsg);
    sleep(Duration::from_millis(300)).await;

    ctx.terminate().await;
}
