use std::{str::FromStr, time::Instant};

use iroh::{Endpoint, PublicKey, dns::DnsResolver, endpoint::presets};
use ping_pong::{Ping, PingPong};
use theta::prelude::*;
use tracing_subscriber::EnvFilter;
use url::Url;

const WARMUP_ITERATIONS: usize = 10_000;
const BENCHMARK_ITERATIONS: usize = 100_000;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    println!("Initializing RootContext...");

    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .dns_resolver(dns)
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();
    println!("RootContext initialized with public key: {public_key}");

    println!("Spawning PingPong actor...");
    let ping_pong = ctx.spawn(PingPong);

    println!("Binding PingPong actor to 'ping_pong' name...");
    let _ = ctx.bind("ping_pong", ping_pong);

    println!("Please enter the public key of the other peer:");
    let other_public_key = tokio::task::spawn_blocking(|| {
        let mut input = String::new();
        loop {
            std::io::stdin()
                .read_line(&mut input)
                .expect("failed to read stdin");
            let trimmed = input.trim();
            if trimmed.is_empty() {
                eprintln!("Public key cannot be empty. Please try again.");
                input.clear();
                continue;
            }
            match PublicKey::from_str(trimmed) {
                Err(e) => {
                    eprintln!("Invalid public key format: {e}");
                    input.clear();
                }
                Ok(public_key) => break public_key,
            };
        }
    })
    .await?;

    let ping_pong_url = Url::parse(&format!("iroh://ping_pong@{other_public_key}"))?;
    let other_ping_pong = match ActorRef::<PingPong>::lookup(&ping_pong_url).await {
        Err(e) => {
            eprintln!("Failed to find PingPong actor at URL: {ping_pong_url}. Error: {e}");
            return Ok(());
        }
        Ok(actor) => actor,
    };

    println!("Starting ping-pong benchmark...");
    println!(
        "Pre-allocating storage for {} measurements...",
        BENCHMARK_ITERATIONS
    );

    let mut latencies = Vec::with_capacity(BENCHMARK_ITERATIONS);

    let ping = Ping { source: public_key };

    println!("Warming up with {WARMUP_ITERATIONS} requests...");
    for _ in 0..WARMUP_ITERATIONS {
        let _ = other_ping_pong.ask(ping.clone()).await;
    }
    println!("Warmup done");

    println!("Starting benchmark with {BENCHMARK_ITERATIONS} requests...");
    let benchmark_start = Instant::now();

    for _ in 0..BENCHMARK_ITERATIONS {
        let start = Instant::now();

        match other_ping_pong.ask(ping.clone()).await {
            Ok(_pong) => {
                latencies.push(start.elapsed().as_nanos() as u64);
            }
            Err(e) => {
                eprintln!("Benchmark error: {e}");
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
        latencies.sort_unstable();

        let min_ns = latencies[0];
        let max_ns = latencies[latencies.len() - 1];
        let mean_ns = latencies.iter().sum::<u64>() / latencies.len() as u64;

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

        println!("\n=== THROUGHPUT ANALYSIS ===");
        let avg_throughput = completed_requests as f64 / total_duration.as_secs_f64();
        println!("Average throughput: {:.2} req/sec", avg_throughput);
        println!("Average latency: {:.2} μs", mean_ns as f64 / 1000.0);
        println!(
            "Theoretical max throughput: {:.2} req/sec",
            1_000_000.0 / (mean_ns as f64 / 1000.0)
        );
    }

    tokio::time::sleep(std::time::Duration::from_secs(30)).await;

    Ok(())
}
