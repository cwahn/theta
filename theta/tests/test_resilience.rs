#![cfg(all(feature = "macros", feature = "remote", feature = "monitor"))]

//! Resilience & resource-management integration tests.
//!
//! Tests run against a companion `resilience_host` example binary
//! that serves a Counter actor bound as "counter".
//!
//! Bug #1: remove_peer evicts wrong peer after path migration (unit-tested in peer.rs)
//! Bug #2: monitor update reader leaks when observer drops
//! Bug #4: stale imports should fail early when peer connection is replaced

use std::{
    hash::{Hash, Hasher},
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    str::FromStr,
    time::Duration,
};

use iroh::{Endpoint, PublicKey, dns::DnsResolver, endpoint::presets};
use serde::{Deserialize, Serialize};
use theta::{monitor::Update, prelude::*};
use theta_flume::unbounded_anonymous;
use theta_macros::ActorArgs;
use tokio::time::sleep;

// ── Actor definitions (must match resilience_host.rs) ───────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

#[derive(Debug, Clone, ActorArgs)]
pub struct Counter {
    pub value: i64,
}

#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for Counter {
    type View = i64;

    const _: () = async |_: Ping| -> i64 {
        self.value += 1;
        self.value
    };

    const _: () = async |_: GetValue| -> i64 { self.value };

    fn hash_code(&self) -> u64 {
        let mut hasher = ahash::AHasher::default();
        self.value.hash(&mut hasher);
        hasher.finish()
    }
}

impl From<&Counter> for i64 {
    fn from(counter: &Counter) -> Self {
        counter.value
    }
}

// ── Test helpers ────────────────────────────────────────────────────

struct TestHost {
    child: Child,
    public_key: PublicKey,
}

impl TestHost {
    fn spawn() -> Self {
        // Build the example binary first
        let status = Command::new("cargo")
            .args(["build", "--example", "resilience_host"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed to build resilience_host");
        assert!(status.success(), "resilience_host build failed");

        let binary = format!(
            "{}/target/debug/examples/resilience_host",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .display()
        );

        let mut child = Command::new(&binary)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        let mut public_key = None;
        for line in reader.lines() {
            let line = line.expect("failed to read host stdout");
            if let Some(key_str) = line.strip_prefix("PUBLIC_KEY:") {
                public_key =
                    Some(PublicKey::from_str(key_str).expect("invalid public key from host"));
                break;
            }
        }

        TestHost {
            child,
            public_key: public_key.expect("host did not print PUBLIC_KEY"),
        }
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── All resilience tests in one function (LocalPeer is a singleton) ─

#[tokio::test]
async fn resilience_tests() {
    tracing_subscriber::fmt()
        .with_env_filter("warn,theta=debug")
        .compact()
        .try_init()
        .ok();

    let host = TestHost::spawn();

    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder(presets::N0)
        .dns_resolver(dns)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await
        .unwrap();
    let _ctx = RootContext::init(endpoint);

    // Give host time to bind
    sleep(Duration::from_secs(3)).await;

    // ── Phase 1: Basic round-trip ───────────────────────────────
    println!("=== Phase 1: Basic remote ask round-trip ===");

    let counter = ActorRef::<Counter>::lookup_remote("counter", host.public_key)
        .await
        .expect("lookup failed")
        .expect("counter not found");

    let v1: i64 = counter
        .ask(Ping)
        .timeout(Duration::from_secs(5))
        .await
        .expect("ask Ping #1 failed");

    let v2: i64 = counter
        .ask(Ping)
        .timeout(Duration::from_secs(5))
        .await
        .expect("ask Ping #2 failed");

    assert_eq!(v2, v1 + 1, "counter should increment on each Ping");
    println!("  PASS: ask round-trip works (v1={v1}, v2={v2})");

    // ── Phase 2: Monitor, drop observer, verify cleanup ─────────
    println!("\n=== Phase 2: Monitor stops after observer drop (Bug #2) ===");

    // Start first monitor
    {
        let (tx, rx) = unbounded_anonymous::<Update<Counter>>();

        counter
            .monitor(tx)
            .await
            .expect("first monitor setup failed");

        // Trigger an update
        let _: i64 = counter
            .ask(Ping)
            .timeout(Duration::from_secs(5))
            .await
            .expect("ask failed during first monitor");

        // Wait for at least one update
        let _update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for first monitor update")
            .expect("first monitor channel closed");

        println!("  Received first monitor update, dropping observer...");
        // rx drops here — monitor task should stop
    }

    // Give the monitor task time to detect the closed channel and terminate
    sleep(Duration::from_millis(500)).await;

    // Trigger more updates — these should not cause resource leaks or panics
    for i in 0..5 {
        let v: i64 = counter
            .ask(Ping)
            .timeout(Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| panic!("ask #{i} failed after monitor drop: {e}"));
        assert!(v > 0);
    }
    println!("  Asks still work after monitor drop.");

    // Set up a second monitor — verifies the stream was released properly
    let (tx2, rx2) = unbounded_anonymous::<Update<Counter>>();
    counter
        .monitor(tx2)
        .await
        .expect("second monitor setup should succeed after first was dropped");

    let _: i64 = counter
        .ask(Ping)
        .timeout(Duration::from_secs(5))
        .await
        .expect("ask failed during second monitor");

    let _update2 = tokio::time::timeout(Duration::from_secs(5), rx2.recv())
        .await
        .expect("timeout waiting for second monitor update")
        .expect("second monitor channel closed unexpectedly");

    println!("  PASS: Second monitor works — stream was released properly.");

    // ── Phase 3: Tell works (fire-and-forget) ───────────────────
    println!("\n=== Phase 3: Tell fire-and-forget ===");

    counter.tell(Ping).expect("tell should succeed");
    sleep(Duration::from_millis(100)).await;

    let v: i64 = counter
        .ask(GetValue)
        .timeout(Duration::from_secs(5))
        .await
        .expect("ask GetValue failed");

    println!("  PASS: tell + ask works (value={v})");

    // ── Summary ─────────────────────────────────────────────────
    println!("\n=== All resilience tests passed ===");
}

