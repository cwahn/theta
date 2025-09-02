use iroh::{Endpoint, PublicKey};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, time::Instant, vec};
use theta::prelude::*;
use theta_macros::ActorArgs;
use url::Url;

#[derive(Debug, Clone, ActorArgs)]
pub struct PingPong;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping {
    pub source: PublicKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pong {}

#[actor("f68fe56f-8aa9-4f90-8af8-591a06e2818a")]
impl Actor for PingPong {
    const _: () = {
        async |_msg: Ping| -> Pong {
            // No logging in benchmark mode
            Pong {}
        };
    };
}

const BENCHMARK_ITERATIONS: usize = 100_000;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Minimal logging setup - only errors
    tracing_log::LogTracer::init().ok();
    tracing_subscriber::fmt()
        .with_env_filter("error")
        .compact()
        .init();

    println!("Initializing RootContext...");
    let endpoint = Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();
    println!("RootContext initialized with public key: {public_key}");

    println!("Spawning PingPong actor...");
    let ping_pong = ctx.spawn(PingPong);

    println!("Binding PingPong actor to 'ping_pong' name...");
    ctx.bind(b"ping_pong", ping_pong);

    // Get other peer's public key
    println!("Please enter the public key of the other peer:");
    let mut input = String::new();
    let other_public_key = loop {
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            eprintln!("Public key cannot be empty. Please try again.");
            input.clear();
            continue;
        }
        match PublicKey::from_str(trimmed) {
            Ok(key) => break key,
            Err(e) => {
                eprintln!("Invalid public key format: {e}");
                input.clear();
            }
        };
    };

    let ping_pong_url = Url::parse(&format!("iroh://ping_pong@{other_public_key}"))?;
    let other_ping_pong = match ActorRef::<PingPong>::lookup(&ping_pong_url).await {
        Ok(actor) => actor,
        Err(e) => {
            eprintln!("Failed to find PingPong actor at URL: {ping_pong_url}. Error: {e}");
            return Ok(());
        }
    };

    println!("Starting 100k ping-pong benchmark...");
    println!(
        "Pre-allocating storage for {} measurements...",
        BENCHMARK_ITERATIONS
    );

    // Pre-allocate storage for timing measurements
    let mut latencies = Vec::with_capacity(BENCHMARK_ITERATIONS);

    // Pre-create the ping message to avoid allocation overhead
    let ping = Ping {
        source: public_key.clone(),
    };

    // Warm-up phase
    println!("Warming up with 10k requests...");
    for _ in 0..10000 {
        let _ = other_ping_pong.ask(ping.clone()).await;
    }

    println!("Starting benchmark...");
    let benchmark_start = Instant::now();

    // Benchmark loop
    for i in 0..BENCHMARK_ITERATIONS {
        let start = Instant::now();

        match other_ping_pong.ask(ping.clone()).await {
            Ok(_pong) => {
                let latency = start.elapsed();
                latencies.push(latency.as_nanos() as u64);
            }
            Err(e) => {
                eprintln!("Failed to send ping at iteration {}: {}", i + 1, e);
                break;
            }
        }
    }

    let total_duration = benchmark_start.elapsed();
    let completed_requests = latencies.len();

    println!("\n=== BENCHMARK RESULTS ===");
    println!("Total requests completed: {}", completed_requests);
    println!("Total time: {:.2?}", total_duration);
    println!(
        "Requests per second: {:.2}",
        completed_requests as f64 / total_duration.as_secs_f64()
    );

    if !latencies.is_empty() {
        // Calculate statistics
        latencies.sort_unstable();

        let min_ns = latencies[0];
        let max_ns = latencies[latencies.len() - 1];
        let mean_ns = latencies.iter().map(|&x| x as u64).sum::<u64>() / latencies.len() as u64;

        // Percentiles
        let p50_ns = latencies[latencies.len() * 50 / 100];
        let p95_ns = latencies[latencies.len() * 95 / 100];
        let p99_ns = latencies[latencies.len() * 99 / 100];
        let p999_ns = latencies[latencies.len() * 999 / 1000];

        println!("\n=== LATENCY STATISTICS (microseconds) ===");
        println!("Min:    {:.2} us", min_ns as f64 / 1000.0);
        println!("Mean:   {:.2} us", mean_ns as f64 / 1000.0);
        println!("Max:    {:.2} us", max_ns as f64 / 1000.0);
        println!("P50:    {:.2} us", p50_ns as f64 / 1000.0);
        println!("P95:    {:.2} us", p95_ns as f64 / 1000.0);
        println!("P99:    {:.2} us", p99_ns as f64 / 1000.0);
        println!("P99.9:  {:.2} us", p999_ns as f64 / 1000.0);

        // Calculate standard deviation
        let variance = latencies
            .iter()
            .map(|&x| {
                let diff = x as f64 - mean_ns as f64;
                diff * diff
            })
            .sum::<f64>()
            / latencies.len() as f64;
        let std_dev_ns = variance.sqrt();

        println!("StdDev: {:.2}", std_dev_ns / 1000.0);

        // Throughput analysis
        println!("\n=== THROUGHPUT ANALYSIS ===");
        let avg_throughput = completed_requests as f64 / total_duration.as_secs_f64();
        println!("Average throughput: {:.2} req/sec", avg_throughput);
        println!("Average latency: {:.2} μs", mean_ns as f64 / 1000.0);
        println!(
            "Theoretical max throughput: {:.2} req/sec",
            1_000_000.0 / (mean_ns as f64 / 1000.0)
        );
    }

    Ok(())
}
