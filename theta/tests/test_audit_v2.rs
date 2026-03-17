#![cfg(feature = "macros")]

//! Audit V2: Tests documenting fixed bugs from second audit.
//!
//! Bug #1: Init-failure deadlocks parent (wait_signal drops Notify)
//! Bug #2: RootContext::terminate() returns before children finish  
//! Bug #3: Lazy cleanup of dead WeakActorHdl in child_hdls
//!
//! All tests now pass with fixes applied.

use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Once,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
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
// BUG #1: Init-failure causes parent deadlock
//
// When a child actor panics during initialize(), it escalates
// to its parent. The parent's default supervision strategy
// sends Signal::Terminate back to the child. But the child is
// in ActorConfig::wait_signal() which only handles Restart —
// all other signals (including Terminate) are dropped without
// acknowledging the Notify. The parent hangs forever on
// join_all(sig_ks).await inside supervise().
//
// File: actor_instance.rs, wait_signal() ~line 165
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
            // After spawning, the child will panic during init,
            // escalate to this parent, and default supervision
            // sends Terminate back — causing deadlock.
        };

        // This handler should still be reachable after supervision
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

#[tokio::test]
async fn bug1_init_failure_deadlocks_parent() {
    init_tracing();
    let ctx = RootContext::default();

    let child_spawned = Arc::new(AtomicBool::new(false));

    let parent = ctx.spawn(ParentOfFailChild {
        child_spawned: child_spawned.clone(),
    });

    // Tell parent to spawn the failing child
    let _ = parent.tell(SpawnFailChild);
    sleep(Duration::from_millis(500)).await;

    // The parent should have spawned the child
    assert!(
        child_spawned.load(Ordering::SeqCst),
        "parent should have spawned the child"
    );

    // BUG: The parent is now deadlocked in supervise().
    // It cannot process any more messages.
    // If we ask with a timeout, it should respond — but it won't.
    let result = parent.ask(Ping).timeout(Duration::from_secs(2)).await;

    assert!(
        result.is_ok(),
        "Parent should still be responsive after child init failure, but it is deadlocked. Got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), 42);
}

// ============================================================
// BUG #2: RootContext::terminate() returns before children finish
//
// RootContext's supervision task sends Terminate(None) to all
// children (fire-and-forget, no Notify), then immediately
// notify_one() to the caller. The caller's terminate().await
// returns before children have actually run on_exit().
//
// Compare with actor-level signal_children() which creates
// SignalRequests with Notify channels and join_all()s them.
//
// File: context.rs, Default::default() ~line 340-350
// ============================================================

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
        // Simulate slow cleanup (e.g., flushing data, closing connections)
        sleep(Duration::from_millis(500)).await;
        self.exited.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn bug2_root_terminate_does_not_wait_for_children() {
    init_tracing();
    let ctx = RootContext::default();

    let exited = Arc::new(AtomicBool::new(false));

    let actor = ctx.spawn(SlowExitActor {
        exited: exited.clone(),
    });

    // Ensure actor is fully started
    let _ = actor.tell(Noop2);
    sleep(Duration::from_millis(100)).await;

    // Terminate and await — should wait for children to finish
    ctx.terminate().await;

    // BUG: terminate().await returns immediately, before on_exit() completes.
    // The exited flag should be true if terminate actually waited.
    assert!(
        exited.load(Ordering::SeqCst),
        "on_exit() should have completed before terminate().await returns, but it hasn't"
    );
}

// ============================================================
// BUG #3: Terminated actor's parent never cleans child_hdls
//
// When an actor is dropped (all ActorRefs gone), it sends
// ChildDropped to its parent, triggering cleanup of child_hdls.
// But when an actor is terminated via signal, terminate() does
// NOT send ChildDropped. The parent's child_hdls Vec accumulates
// dead WeakActorHdl entries that are never pruned.
//
// File: actor_instance.rs, terminate() ~line 435
//       vs drop() ~line 425 which DOES send ChildDropped
// ============================================================

/// This test demonstrates that parent's child_hdls gets lazily cleaned
/// after children are terminated. Previously, terminated children did
/// not send ChildDropped and the Vec grew unbounded. With lazy cleanup,
/// dead entries are pruned during signal_children/supervise iteration.
///
/// We verify that:
/// 1. Dropped actor's on_exit fires (normal drop path with ChildDropped)
/// 2. Terminated actor's on_exit fires (root terminate path)
/// 3. Root terminate completes cleanly without hanging on dead children
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

    // Ensure both are started
    let _ = actor1.tell(Noop3);
    let _ = actor2.tell(Noop3);
    sleep(Duration::from_millis(100)).await;

    // Drop actor1 — triggers ChildDropped → root cleans child_hdls entry
    drop(actor1);
    sleep(Duration::from_millis(200)).await;
    assert!(
        exited1.load(Ordering::SeqCst),
        "dropped actor should have called on_exit"
    );

    // actor2 is still alive. Root's child_hdls has 1 dead + 1 alive entry.
    // Root terminate should signal only alive children and complete cleanly.
    // With lazy cleanup, the dead entry is pruned during iteration.
    ctx.terminate().await;

    assert!(
        exited2.load(Ordering::SeqCst),
        "terminated actor should have called on_exit via root terminate"
    );
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
