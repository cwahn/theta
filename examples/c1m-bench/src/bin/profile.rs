#![allow(clippy::cast_precision_loss, clippy::too_many_lines)]

use std::{
    str::FromStr,
    time::{Duration, Instant},
};

use hdrhistogram::Histogram;
use iroh::{
    Endpoint, PublicKey,
    dns::DnsResolver,
    endpoint::{QuicTransportConfig, VarInt, presets},
};
use sysinfo::{System, get_current_pid};
use theta::prelude::*;
use theta_c1m_bench::{GetWorkers, Manager, Ping, Worker};
use tracing::info;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::{fmt::time::ChronoLocal, prelude::*};
use url::Url;

fn memory_usage_mb() -> f64 {
    let mut sys = System::new();

    let pid = get_current_pid().expect("failed to get PID");

    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);

    sys.process(pid)
        .map_or(0.0, |p| p.memory() as f64 / 1024.0 / 1024.0)
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let trace_path = std::env::var("TRACE_FILE").unwrap_or_else(|_| "trace.json".to_string());
    let (chrome_layer, _chrome_guard) = ChromeLayerBuilder::new()
        .file(&trace_path)
        .include_args(true)
        .build();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
                .compact()
                .with_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,theta=warn")),
                ),
        )
        .with(chrome_layer)
        .init();
    tracing_log::LogTracer::init().ok();
    println!("Trace output: {trace_path} (open at https://ui.perfetto.dev)");

    let host_pk = if let Some(key_str) = std::env::args().nth(1) {
        PublicKey::from_str(&key_str)?
    } else {
        eprintln!("Usage: c1m_profile <SERVER_PUBLIC_KEY>");
        std::process::exit(1);
    };

    let expected_n: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);
    let max_streams: u32 = std::env::var("MAX_STREAMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(expected_n + 10);
    let sample_size: usize = std::env::var("SAMPLE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let transport_config = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(VarInt::from_u32(max_streams))
        .max_concurrent_bidi_streams(VarInt::from_u32(max_streams))
        .build();
    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder(presets::N0)
        .dns_resolver(dns)
        .transport_config(transport_config)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;
    let _ctx = RootContext::init(endpoint);
    let separator = "=".repeat(70);

    println!("\n{separator}");
    println!("  C1M Profiling Benchmark");
    println!("{separator}");

    let url = Url::parse(&format!("iroh://manager@{host_pk}"))?;

    info!("looking up manager at {url}");

    let t_lookup = Instant::now();
    let manager = ActorRef::<Manager>::lookup(&url).await?;
    let lookup_dur = t_lookup.elapsed();

    println!("\n[1. Manager Lookup]");
    println!("  Connected in {lookup_dur:?}");

    let mem_before_workers = memory_usage_mb();
    let t_get = Instant::now();
    let workers: Vec<ActorRef<Worker>> = manager
        .ask(GetWorkers)
        .timeout(Duration::from_mins(2))
        .await?;
    let get_dur = t_get.elapsed();
    let n = workers.len();
    let mem_after_workers = memory_usage_mb();

    println!("\n[2. GetWorkers]");
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

    println!("\n[2b. Import Pre-warm]");

    {
        let t_warm = Instant::now();

        for worker in &workers {
            let _ = worker.tell(Ping);
        }

        println!(
            "  Sent {n} tell(Ping) to trigger lazy stream opening in {:.1}ms",
            t_warm.elapsed().as_secs_f64() * 1000.0
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    println!("\n[3. Warmup]");

    let warmup_count = 10.min(n);

    let mut warmup_hist = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();

    for (i, worker) in workers.iter().enumerate().take(warmup_count) {
        let t = Instant::now();

        match worker.ask(Ping).timeout(Duration::from_secs(30)).await {
            Ok(_) => {
                let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
                let _ = warmup_hist.record(us);
            }
            Err(e) => println!("  warmup {i} failed: {e}"),
        }
    }

    print_histogram("Warmup latency", &warmup_hist);
    println!("\n[4. Sequential Ask (sampled latency distribution)]");

    let seq_sample = sample_size.min(n);
    let step = n.checked_div(seq_sample).unwrap_or(1);

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
                let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
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
    println!("\n[5. Sequential Batch Analysis]");

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

                if workers[idx]
                    .ask(Ping)
                    .timeout(Duration::from_secs(10))
                    .await
                    .is_err()
                {
                    continue;
                }

                let _ =
                    batch_hist.record(u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX));
            }

            if batch_hist.is_empty() {
                continue;
            }

            batch_means.push(batch_hist.mean());
            batch_p99s.push(batch_hist.value_at_quantile(0.99) as f64);
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

    println!();
    println!("\n[6. Concurrent Ask - Wave Analysis]");

    let wave_sizes = [100, 1000, 5000, 10000, 50_000, 100_000, n];

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

        let latencies = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(wave_n)));

        for worker in workers.iter().take(wave_n) {
            let worker = worker.clone();
            let lat_collector = latencies.clone();

            handles.push(tokio::spawn(async move {
                let t = Instant::now();
                let result = worker.ask(Ping).timeout(Duration::from_mins(1)).await;
                let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);

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
        let lats: Vec<u64> = latencies.lock().unwrap().to_vec();

        let mut wave_hist = Histogram::<u64>::new_with_bounds(1, 120_000_000, 3).unwrap();

        for &us in &lats {
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

    println!("\n[7. Tell Throughput]");

    let tell_batch_sizes = [1000, 10000, 50_000, 100_000, n];

    for &tn in &tell_batch_sizes {
        if tn > n {
            continue;
        }

        if tn == 0 {
            continue;
        }

        let t = Instant::now();

        let mut ok = 0usize;

        for worker in workers.iter().take(tn) {
            if worker.tell(Ping).is_ok() {
                ok += 1;
            }
        }

        let dur = t.elapsed();

        println!(
            "  N={tn}: {ok}/{tn} in {dur:?} ({:.0} msgs/s)",
            ok as f64 / dur.as_secs_f64()
        );
    }

    let mem_final = memory_usage_mb();

    println!("\n[8. Memory Profile]");
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

    println!("\n{separator}");
    println!("  Trace written to: {trace_path}");
    println!("  View at https://ui.perfetto.dev or chrome://tracing");
    println!("{separator}\n");

    Ok(())
}
