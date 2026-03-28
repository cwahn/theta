use std::time::Instant;

use iroh::{
    Endpoint,
    dns::DnsResolver,
    endpoint::{QuicTransportConfig, VarInt, presets},
};
use sysinfo::System;
use theta::prelude::*;
use theta_c1m_bench::{Manager, Worker};
use tracing::info;
use tracing_subscriber::fmt::time::ChronoLocal;

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
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,theta=info")),
        )
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .compact()
        .init();
    tracing_log::LogTracer::init().ok();

    let n: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "100".to_string())
        .parse()
        .expect("invalid N");

    let mem_before = memory_usage_mb();

    let max_streams: u32 = std::env::var("MAX_STREAMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024);

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

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();

    info!("spawning {n} workers...");
    let t_spawn = Instant::now();
    let mut workers = Vec::with_capacity(n);
    for i in 0..n {
        workers.push(ctx.spawn(Worker { id: i as u64 }));
    }
    let spawn_dur = t_spawn.elapsed();
    let mem_after = memory_usage_mb();

    info!("spawned {n} workers in {spawn_dur:?} (mem: {mem_before:.1} -> {mem_after:.1} MB)");

    let manager = ctx.spawn(Manager { workers });
    ctx.bind("manager", manager)?;

    println!("PUBLIC_KEY:{public_key}");
    println!("Server ready with {n} workers. Ctrl-C to stop.");

    tokio::signal::ctrl_c().await.ok();
    println!("\nReceived Ctrl-C, dumping perf stats...");
    theta::perf_instrument::dump_perf_stats();
    Ok(())
}
