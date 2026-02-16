#![cfg(feature = "macros")]

//! Audit V2: Failing tests that reveal newly discovered bugs.
//!
//! Each test is designed to FAIL under the current implementation,
//! demonstrating the bug. Once the bug is fixed, the test should pass.
//!
//! DO NOT fix the source code — these tests document the bugs.

use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Once,
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
        async |Ping: Ping| -> u64 {
            42
        };
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
    let result = parent
        .ask(Ping)
        .timeout(Duration::from_secs(2))
        .await;

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

// ============================================================
// BUG #4: Stale bindings after actor termination
//
// When an actor is bound via ctx.bind("name", actor), the
// BINDINGS DashMap stores a strong ActorRef. After the actor is
// terminated, the binding persists. lookup_local() succeeds but
// returns a dead ActorRef whose tell()/ask() will fail.
//
// Additionally, the strong ActorRef in BINDINGS prevents the
// actor from ever entering the Drop path — it can only be
// stopped via explicit Terminate.
//
// File: context.rs, bind() and BINDINGS static
// ============================================================

#[derive(Debug, Clone)]
pub struct BoundActor;

impl ActorArgs for BoundActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundPing;

#[actor("77777777-7777-7777-7777-777777777777")]
impl Actor for BoundActor {
    const _: () = {
        async |BoundPing: BoundPing| -> u64 {
            99
        };
    };
}

#[tokio::test]
async fn bug4_stale_binding_after_termination() {
    init_tracing();
    let ctx = RootContext::default();

    let actor = ctx.spawn(BoundActor);
    ctx.bind("test_bound", actor.clone()).unwrap();

    // Verify lookup works while actor is alive
    let found = ActorRef::<BoundActor>::lookup_local("test_bound");
    assert!(found.is_ok(), "lookup should succeed while actor is alive");

    let response = found.unwrap().ask(BoundPing).timeout(Duration::from_secs(1)).await;
    assert_eq!(response.unwrap(), 99);

    // Terminate the actor
    ctx.terminate().await;
    sleep(Duration::from_millis(300)).await;

    // BUG: lookup still succeeds, returning a dead ActorRef
    let found_after = ActorRef::<BoundActor>::lookup_local("test_bound");

    // After termination, lookup should fail (return NotFound)
    // because finding a dead actor is worse than finding nothing.
    assert!(
        found_after.is_err(),
        "lookup should fail after actor is terminated, but it returned a stale reference"
    );
}

// ============================================================
// BUG #5: Bound actor cannot be dropped naturally
//
// The BINDINGS DashMap holds a strong ActorRef (which owns a
// strong MsgTx sender). As long as this sender exists, the
// actor's msg_rx.recv() never returns None, meaning the actor
// will never enter the Drop path — even if ALL user-held
// ActorRefs are dropped.
//
// The actor is effectively immortal until explicitly terminated.
// This is surprising: dropping all handles to a bound actor
// should eventually let it shut down cleanly.
//
// File: context.rs, bind_impl() uses Arc<ActorRef>
//       actor_instance.rs, process() checks msg_rx.recv()
// ============================================================

#[derive(Debug, Clone)]
pub struct ImmortalActor {
    exited: Arc<AtomicBool>,
}

impl ActorArgs for ImmortalActor {
    type Actor = Self;
    async fn initialize(_ctx: Context<Self>, args: &Self) -> Self {
        args.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmortalPing;

#[actor("88888888-8888-8888-8888-888888888888")]
impl Actor for ImmortalActor {
    const _: () = {
        async |ImmortalPing: ImmortalPing| {};
    };

    async fn on_exit(&mut self, _code: ExitCode) {
        self.exited.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn bug5_bound_actor_cannot_drop_naturally() {
    init_tracing();
    let ctx = RootContext::default();

    let exited = Arc::new(AtomicBool::new(false));

    let actor = ctx.spawn(ImmortalActor {
        exited: exited.clone(),
    });

    // Ensure it's started
    let _ = actor.tell(ImmortalPing);
    sleep(Duration::from_millis(100)).await;

    // Bind the actor
    ctx.bind("immortal", actor.clone()).unwrap();

    // Now drop ALL user-held references
    drop(actor);

    // Wait for potential drop
    sleep(Duration::from_millis(500)).await;

    // BUG: Actor should have exited via on_exit(ExitCode::Dropped),
    // but the strong ActorRef in BINDINGS keeps msg_rx alive.
    assert!(
        exited.load(Ordering::SeqCst),
        "Bound actor should be droppable when all user references are released, \
         but the BINDINGS strong reference keeps it alive indefinitely"
    );
}
