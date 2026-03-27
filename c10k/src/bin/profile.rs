use std::{
    str::FromStr,
    time::{Duration, Instant},
};

use hdrhistogram::Histogram;
use iroh::{
    Endpoint, PublicKey,
    endpoint::{QuicTransportConfig, VarInt, presets},
};
use sysinfo::System;
use theta::prelude::*;
use theta_c10k::{GetWorkers, Manager, Ping, Worker};
use tracing::info;
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

fn print_histogram(name: &str, hist: &Histogram<u64>) {
    if hist.is_empty() {
        println!("  {name}: (empty)");
        return;
    }
    let mean = hist.mean();
    let stdev = hist.stdev();
    println!("  {name}:");
    println!("    count:  {}", hist.len());
    println!("    min:    {} µs", hist.min());
    println!("    p50:    {} µs", hist.value_at_quantile(0.50));
    println!("    p90:    {} µs", hist.value_at_quantile(0.90));
    println!("    p95:    {} µs", hist.value_at_quantile(0.95));
    println!("    p99:    {} µs", hist.value_at_quantile(0.99));
    println!("    p999:   {} µs", hist.value_at_quantile(0.999));
    println!("    max:    {} µs", hist.max());
    println!("    mean:   {mean:.1} µs");
    println!("    stdev:  {stdev:.1} µs");
    println!(
        "    cv:     {:.2}%",
        if mean > 0.0 {
            stdev / mean * 100.0
        } else {
            0.0
        }
    );
}

/// Wait until the connection to the given node is direct (not relayed).
/// Returns the elapsed wait time.
async fn wait_for_direct(
    ctx: &RootContext,
    node_id: PublicKey,
    timeout: Duration,
) -> Result<Duration, String> {
    let start = Instant::now();
    let endpoint = ctx.endpoint();

    loop {
        if start.elapsed() > timeout {
            return Err(format!(
                "timed out after {:?} waiting for direct connection",
                timeout
            ));
        }
        if let Some(info) = endpoint.remote_info(node_id).await {
            // Check if any active address is a direct IP (not relay)
            let has_direct = info
                .addrs()
                .any(|addr_info: &iroh::endpoint::TransportAddrInfo| addr_info.addr().is_ip());
            if has_direct {
                return Ok(start.elapsed());
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,theta=warn")),
        )
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .compact()
        .init();
    tracing_log::LogTracer::init().ok();

    let host_pk = match std::env::args().nth(1) {
        Some(key_str) => PublicKey::from_str(&key_str)?,
        None => {
            eprintln!("Usage: c1m_profile <SERVER_PUBLIC_KEY>");
            std::process::exit(1);
        }
    };

    let max_streams: u32 = std::env::var("MAX_STREAMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let sample_size: usize = std::env::var("SAMPLE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    let transport_config = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(VarInt::from_u32(max_streams))
        .build();

    let endpoint = Endpoint::builder(presets::N0)
        .transport_config(transport_config)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);

    let sep = "=".repeat(70);
    println!("\n{sep}");
    println!("  C1M Profiling Benchmark");
    println!("{sep}");

    // ── Phase 1: Connect + Lookup Manager ──
    let url = Url::parse(&format!("iroh://manager@{host_pk}"))?;
    info!("looking up manager at {url}");
    let t_lookup = Instant::now();
    let manager = ActorRef::<Manager>::lookup(&url).await?;
    let lookup_dur = t_lookup.elapsed();
    println!("\n[1. Manager Lookup]");
    println!("  Connected in {lookup_dur:?}");

    // ── Phase 2: Wait for direct connection ──
    println!("\n[2. Connection Type Check]");
    print!("  Waiting for direct connection...");
    match wait_for_direct(&ctx, host_pk, Duration::from_secs(30)).await {
        Ok(wait_dur) => {
            println!(" DIRECT after {wait_dur:?}");
        }
        Err(e) => {
            println!(" RELAY ONLY ({e})");
            println!("  WARNING: Running over relay — latencies will be higher.");
            println!(
                "  (Both processes run locally, so this is expected if hole-punching can't work.)"
            );
        }
    }

    // Verify actual path being used with a test ping
    let t_verify = Instant::now();
    let _ = manager
        .ask(GetWorkers)
        .timeout(Duration::from_secs(60))
        .await?;
    println!("  Verification ask: {:?}", t_verify.elapsed());

    // ── Phase 3: Get Workers ──
    let mem_before_workers = memory_usage_mb();
    let t_get = Instant::now();
    let workers: Vec<ActorRef<Worker>> = manager
        .ask(GetWorkers)
        .timeout(Duration::from_secs(120))
        .await?;
    let get_dur = t_get.elapsed();
    let n = workers.len();
    let mem_after_workers = memory_usage_mb();

    println!("\n[3. GetWorkers]");
    println!("  Count: {n}");
    println!("  Duration: {get_dur:?}");
    println!(
        "  Memory: {mem_before_workers:.1} -> {mem_after_workers:.1} MB (+{:.1} MB)",
        mem_after_workers - mem_before_workers
    );
    if n > 0 {
        println!(
            "  Per-ref memory: ~{:.2} KB",
            (mem_after_workers - mem_before_workers) * 1024.0 / n as f64
        );
    }

    // ── Phase 4: Warmup — multiple pings to stabilize ──
    println!("\n[4. Warmup]");
    let warmup_count = 10.min(n);
    let mut warmup_hist = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
    for i in 0..warmup_count {
        let t = Instant::now();
        match workers[i].ask(Ping).timeout(Duration::from_secs(30)).await {
            Ok(_) => {
                let us = t.elapsed().as_micros() as u64;
                let _ = warmup_hist.record(us);
            }
            Err(e) => println!("  warmup {i} failed: {e}"),
        }
    }
    print_histogram("Warmup latency", &warmup_hist);

    // ── Phase 5: Sequential ask — sampled latency distribution ──
    println!("\n[5. Sequential Ask (sampled latency distribution)]");
    let seq_sample = sample_size.min(n);
    // Sample evenly across the worker population
    let step = if seq_sample > 0 { n / seq_sample } else { 1 };
    let mut seq_hist = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
    let mut seq_fail = 0usize;
    let t_seq = Instant::now();
    for i in 0..seq_sample {
        let idx = i * step;
        let t = Instant::now();
        match workers[idx]
            .ask(Ping)
            .timeout(Duration::from_secs(10))
            .await
        {
            Ok(_) => {
                let us = t.elapsed().as_micros() as u64;
                let _ = seq_hist.record(us);
            }
            Err(_) => seq_fail += 1,
        }
    }
    let seq_dur = t_seq.elapsed();
    let seq_success = seq_sample - seq_fail;
    println!("  Sample: {seq_success}/{seq_sample} workers (step={step}, {seq_fail} failures)");
    println!("  Total time: {seq_dur:?}");
    if seq_success > 0 {
        println!(
            "  Throughput: {:.0} rt/s",
            seq_success as f64 / seq_dur.as_secs_f64()
        );
    }
    print_histogram("Sequential latency", &seq_hist);

    // ── Phase 6: Sequential ask — steady-state batches ──
    // Run in batches of 100 to detect variance across the population
    println!("\n[6. Sequential Batch Analysis]");
    let batch_size = 100;
    let num_batches = (seq_sample / batch_size).min(20);
    if num_batches >= 2 {
        let mut batch_means = Vec::new();
        let mut batch_p99s = Vec::new();
        for b in 0..num_batches {
            let mut batch_hist = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
            for j in 0..batch_size {
                let idx = (b * batch_size + j) * step;
                if idx >= n {
                    break;
                }
                let t = Instant::now();
                if let Ok(_) = workers[idx]
                    .ask(Ping)
                    .timeout(Duration::from_secs(10))
                    .await
                {
                    let _ = batch_hist.record(t.elapsed().as_micros() as u64);
                }
            }
            if !batch_hist.is_empty() {
                batch_means.push(batch_hist.mean());
                batch_p99s.push(batch_hist.value_at_quantile(0.99) as f64);
            }
        }
        if batch_means.len() >= 2 {
            let mean_of_means: f64 = batch_means.iter().sum::<f64>() / batch_means.len() as f64;
            let std_of_means: f64 = (batch_means
                .iter()
                .map(|m| (m - mean_of_means).powi(2))
                .sum::<f64>()
                / (batch_means.len() - 1) as f64)
                .sqrt();
            let mean_of_p99: f64 = batch_p99s.iter().sum::<f64>() / batch_p99s.len() as f64;
            let std_of_p99: f64 = (batch_p99s
                .iter()
                .map(|p| (p - mean_of_p99).powi(2))
                .sum::<f64>()
                / (batch_p99s.len() - 1) as f64)
                .sqrt();

            println!("  {num_batches} batches of {batch_size}:");
            println!(
                "  Batch mean latency:  {mean_of_means:.1} ± {std_of_means:.1} µs (CV={:.1}%)",
                if mean_of_means > 0.0 {
                    std_of_means / mean_of_means * 100.0
                } else {
                    0.0
                }
            );
            println!(
                "  Batch p99 latency:   {mean_of_p99:.0} ± {std_of_p99:.0} µs (CV={:.1}%)",
                if mean_of_p99 > 0.0 {
                    std_of_p99 / mean_of_p99 * 100.0
                } else {
                    0.0
                }
            );

            // Detect outlier batches (> 2 std from mean)
            let outliers: Vec<_> = batch_means
                .iter()
                .enumerate()
                .filter(|(_, m)| (**m - mean_of_means).abs() > 2.0 * std_of_means)
                .collect();
            if !outliers.is_empty() {
                println!(
                    "  ⚠ Outlier batches: {:?}",
                    outliers
                        .iter()
                        .map(|(i, m)| format!("batch {i}: {m:.0}µs"))
                        .collect::<Vec<_>>()
                );
            }
        }
    } else {
        println!("  (too few samples for batch analysis)");
    }

    // ── Phase 7: Concurrent ask — wave analysis ──
    // Dump sequential-phase perf stats, then reset for concurrent phase
    println!("\n[6b. Perf Stats — Sequential Phase]");
    theta::perf_instrument::dump_perf_stats();
    theta::perf_instrument::reset_perf_stats();

    println!("\n[7. Concurrent Ask — Wave Analysis]");
    let wave_sizes = [100, 1000, 5000, 10000, 50000, 100000, n];
    for &wave_n in &wave_sizes {
        if wave_n > n {
            continue;
        }
        if wave_n == 0 {
            continue;
        }
        let mem_pre = memory_usage_mb();
        let t_wave = Instant::now();
        let mut handles = Vec::with_capacity(wave_n);

        // Use Arc<Histogram> with mutex for per-task latency collection
        let latencies = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(wave_n)));

        for i in 0..wave_n {
            let worker = workers[i].clone();
            let lat_collector = latencies.clone();
            handles.push(tokio::spawn(async move {
                let t = Instant::now();
                let result = worker.ask(Ping).timeout(Duration::from_secs(60)).await;
                let us = t.elapsed().as_micros() as u64;
                if result.is_ok() {
                    lat_collector.lock().unwrap().push(us);
                }
                result.is_ok()
            }));
        }

        let mut wave_ok = 0usize;
        let mut wave_fail = 0usize;
        for h in handles {
            match h.await {
                Ok(true) => wave_ok += 1,
                _ => wave_fail += 1,
            }
        }
        let wave_dur = t_wave.elapsed();
        let mem_post = memory_usage_mb();

        // Build histogram from collected latencies
        let lats = latencies.lock().unwrap();
        let mut wave_hist = Histogram::<u64>::new_with_bounds(1, 120_000_000, 3).unwrap();
        for &us in lats.iter() {
            let _ = wave_hist.record(us);
        }

        println!("\n  Wave {wave_n}:");
        println!("    {wave_ok}/{wave_n} OK, {wave_fail} failures, {wave_dur:?}");
        if wave_ok > 0 {
            println!(
                "    Throughput: {:.0} rt/s",
                wave_ok as f64 / wave_dur.as_secs_f64()
            );
        }
        println!("    Memory delta: {:.1} MB", mem_post - mem_pre);
        print_histogram("    Concurrent latency", &wave_hist);
    }

    // ── Phase 8: Tell throughput profiling ──
    println!("\n[8. Tell Throughput]");
    let tell_batch_sizes = [1000, 10000, 50000, 100000, n];
    for &tn in &tell_batch_sizes {
        if tn > n {
            continue;
        }
        if tn == 0 {
            continue;
        }
        let t = Instant::now();
        let mut ok = 0usize;
        for i in 0..tn {
            if workers[i].tell(Ping).is_ok() {
                ok += 1;
            }
        }
        let dur = t.elapsed();
        println!(
            "  N={tn}: {ok}/{tn} in {dur:?} ({:.0} msgs/s)",
            ok as f64 / dur.as_secs_f64()
        );
    }

    // ── Phase 9: Memory Profile ──
    let mem_final = memory_usage_mb();
    println!("\n[9. Memory Profile]");
    println!("  Before workers: {mem_before_workers:.1} MB");
    println!("  After workers:  {mem_after_workers:.1} MB");
    println!("  Final:          {mem_final:.1} MB");
    println!("  Total growth:   {:.1} MB", mem_final - mem_before_workers);
    if n > 0 {
        println!(
            "  Per-actor overhead: ~{:.2} KB",
            (mem_final - mem_before_workers) * 1024.0 / n as f64
        );
    }

    // ── Phase 10: Perf Instrumentation Report ──
    theta::perf_instrument::dump_perf_stats();

    println!("\n{sep}\n");
    Ok(())
}
