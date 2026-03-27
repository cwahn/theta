use std::{str::FromStr, time::{Duration, Instant}};

use iroh::{Endpoint, PublicKey, endpoint::{QuicTransportConfig, VarInt, presets}};
use sysinfo::System;
use theta::prelude::*;
use theta_c10k::{GetWorkers, Manager, Ping, Worker};
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

fn memory_usage_mb() -> f64 {
    let mut sys = System::new();
    let pid = sysinfo::get_current_pid().expect("failed to get PID");
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .map(|p| p.memory() as f64 / 1024.0 / 1024.0)
        .unwrap_or(0.0)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,theta=info"))
        )
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .compact()
        .init();
    tracing_log::LogTracer::init().ok();

    // Accept public key as CLI argument
    let host_pk = match std::env::args().nth(1) {
        Some(key_str) => PublicKey::from_str(&key_str)?,
        None => {
            eprintln!("Usage: c10k_client <SERVER_PUBLIC_KEY>");
            std::process::exit(1);
        }
    };

    let max_streams: u32 = std::env::var("MAX_STREAMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let transport_config = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(VarInt::from_u32(max_streams))
        .max_concurrent_bidi_streams(VarInt::from_u32(max_streams))
        .build();

    let endpoint = Endpoint::builder(presets::N0)
        .transport_config(transport_config)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;

    let _ctx = RootContext::init(endpoint);

    let sep = "=".repeat(60);
    let mem_before = memory_usage_mb();

    // Phase 1: Look up the manager remotely
    let url = Url::parse(&format!("iroh://manager@{host_pk}"))?;
    info!("looking up manager at {url}");
    let t_lookup = Instant::now();
    let manager = ActorRef::<Manager>::lookup(&url).await?;
    let lookup_dur = t_lookup.elapsed();
    println!("\n{sep}");
    println!("  Remote C10K Benchmark");
    println!("{sep}");
    println!("\n[Manager Lookup]");
    println!("  Connected in {lookup_dur:?}");

    // Phase 2: Get all worker refs from manager
    let t_get = Instant::now();
    info!("[C10K] Asking manager for workers...");
    let workers: Vec<ActorRef<Worker>> = manager.ask(GetWorkers).timeout(Duration::from_secs(10)).await?;
    let get_dur = t_get.elapsed();
    let n = workers.len();
    info!("[C10K] Got {n} workers in {get_dur:?}");
    println!("\n[GetWorkers]");
    println!("  Received {n} worker refs in {get_dur:?}");

    let mem_after_refs = memory_usage_mb();

    // Warm-up: single ask with timeout to verify connectivity
    println!("\n[Warm-up] Sending single ask to first worker...");
    let t_warmup = Instant::now();
    match workers[0].ask(Ping).timeout(Duration::from_secs(30)).await {
        Ok(id) => println!("  Worker 0 responded with id={id} in {:?}", t_warmup.elapsed()),
        Err(e) => {
            println!("  ERROR: first worker ask failed: {e}");
            println!("  Remote communication may not be working. Aborting.");
            return Ok(());
        }
    }

    // Phase 3: Sequential remote ask to all workers
    let timeout_dur = Duration::from_secs(10);
    let t_ask = Instant::now();
    let mut success = 0usize;
    let mut fail = 0usize;
    for worker in &workers {
        match worker.ask(Ping).timeout(timeout_dur).await {
            Ok(_) => success += 1,
            Err(e) => {
                if fail < 3 {
                    error!("ask failed: {e}");
                }
                fail += 1;
            }
        }
    }
    let ask_dur = t_ask.elapsed();

    println!("\n[Sequential Ask - remote round-trips]");
    println!("  Completed {success}/{n} in {ask_dur:?} ({fail} failures)");
    if n > 0 {
        println!("  Avg latency: {:?}/round-trip", ask_dur / n as u32);
        println!(
            "  Throughput: {:.0} round-trips/sec",
            n as f64 / ask_dur.as_secs_f64()
        );
    }

    // Phase 4: Concurrent remote ask (tokio::spawn all at once)
    let t_concurrent = Instant::now();
    let mut handles = Vec::with_capacity(n);
    for worker in &workers {
        let worker = worker.clone();
        handles.push(tokio::spawn(async move { worker.ask(Ping).timeout(Duration::from_secs(30)).await }));
    }

    let mut concurrent_success = 0usize;
    let mut concurrent_fail = 0usize;
    for handle in handles {
        match handle.await {
            Ok(Ok(_)) => concurrent_success += 1,
            _ => concurrent_fail += 1,
        }
    }
    let concurrent_dur = t_concurrent.elapsed();

    println!("\n[Concurrent Ask - all at once]");
    println!(
        "  Completed {concurrent_success}/{n} in {concurrent_dur:?} ({concurrent_fail} failures)"
    );
    if n > 0 {
        println!(
            "  Avg latency: {:?}/round-trip",
            concurrent_dur / n as u32
        );
        println!(
            "  Throughput: {:.0} round-trips/sec",
            n as f64 / concurrent_dur.as_secs_f64()
        );
    }

    // Phase 5: Tell (fire-and-forget) throughput
    let t_tell = Instant::now();
    let mut tell_ok = 0usize;
    for worker in &workers {
        match worker.tell(Ping) {
            Ok(_) => tell_ok += 1,
            Err(_) => {}
        }
    }
    let tell_dur = t_tell.elapsed();

    println!("\n[Tell - fire-and-forget]");
    println!("  Sent {tell_ok}/{n} in {tell_dur:?}");
    if n > 0 {
        println!(
            "  Throughput: {:.0} msgs/sec",
            n as f64 / tell_dur.as_secs_f64()
        );
    }

    // Memory summary
    let mem_final = memory_usage_mb();
    println!("\n[Memory Summary (client)]");
    println!("  Before:       {mem_before:.2} MB");
    println!("  After refs:   {mem_after_refs:.2} MB");
    println!("  Final:        {mem_final:.2} MB");

    println!("\n{sep}\n");

    Ok(())
}
