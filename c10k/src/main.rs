use std::time::Instant;

use serde::{Deserialize, Serialize};
use sysinfo::System;
use theta::prelude::*;

// --- Actor definition ---

#[derive(Debug, Clone, ActorArgs)]
struct Worker {
    id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ping;

#[actor("a1b2c3d4-e5f6-7890-abcd-ef0123456789")]
impl Actor for Worker {
    const _: () = async |_: Ping| -> u64 { self.id };
}

// --- Benchmark ---

fn memory_usage_mb() -> f64 {
    let mut sys = System::new();
    let pid = sysinfo::get_current_pid().expect("failed to get PID");
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .map(|p| p.memory() as f64 / 1024.0 / 1024.0)
        .unwrap_or(0.0)
}

async fn run_benchmark(n: usize) -> anyhow::Result<()> {
    let sep = "=".repeat(60);
    println!("\n{sep}");
    println!("  C10K Benchmark: N = {n}");
    println!("{sep}");

    let mem_before = memory_usage_mb();

    // Phase 1: Spawn actors
    let t_spawn = Instant::now();
    let ctx = RootContext::init_local();
    let mut actors: Vec<ActorRef<Worker>> = Vec::with_capacity(n);
    for i in 0..n {
        actors.push(ctx.spawn(Worker { id: i as u64 }));
    }
    let spawn_dur = t_spawn.elapsed();
    let mem_after_spawn = memory_usage_mb();

    println!("\n[Spawn]");
    println!("  Spawned {n} actors in {spawn_dur:?}");
    println!("  Avg spawn time: {:?}/actor", spawn_dur / n as u32);
    println!("  Memory: {mem_before:.2} MB -> {mem_after_spawn:.2} MB (delta: {:.2} MB)", mem_after_spawn - mem_before);

    // Phase 2: Tell (fire-and-forget) to all actors
    let t_tell = Instant::now();
    for actor in &actors {
        actor.tell(Ping)?;
    }
    let tell_dur = t_tell.elapsed();

    println!("\n[Tell - fire-and-forget]");
    println!("  Sent {n} tell messages in {tell_dur:?}");
    println!("  Throughput: {:.0} msgs/sec", n as f64 / tell_dur.as_secs_f64());

    // Phase 3: Ask (request-response) to all actors
    let t_ask = Instant::now();
    let mut futures = Vec::with_capacity(n);
    for actor in &actors {
        futures.push(actor.ask(Ping));
    }
    // Await all responses
    let mut success = 0usize;
    let mut fail = 0usize;
    for fut in futures {
        match fut.await {
            Ok(_) => success += 1,
            Err(_) => fail += 1,
        }
    }
    let ask_dur = t_ask.elapsed();
    let mem_after_ask = memory_usage_mb();

    println!("\n[Ask - request-response]");
    println!("  Completed {success}/{n} ask round-trips in {ask_dur:?} ({fail} failures)");
    println!("  Avg latency: {:?}/round-trip", ask_dur / n as u32);
    println!("  Throughput: {:.0} round-trips/sec", n as f64 / ask_dur.as_secs_f64());

    // Phase 4: Concurrent ask using tokio::spawn
    let t_concurrent = Instant::now();
    let mut handles = Vec::with_capacity(n);
    for actor in &actors {
        let actor = actor.clone();
        handles.push(tokio::spawn(async move { actor.ask(Ping).await }));
    }

    let mut concurrent_success = 0usize;
    for handle in handles {
        if let Ok(Ok(_)) = handle.await {
            concurrent_success += 1;
        }
    }
    let concurrent_dur = t_concurrent.elapsed();

    println!("\n[Concurrent Ask - tokio::spawn per request]");
    println!("  Completed {concurrent_success}/{n} concurrent asks in {concurrent_dur:?}");
    println!("  Avg latency: {:?}/round-trip", concurrent_dur / n as u32);
    println!("  Throughput: {:.0} round-trips/sec", n as f64 / concurrent_dur.as_secs_f64());

    // Phase 5: Memory summary
    let mem_final = memory_usage_mb();
    println!("\n[Memory Summary]");
    println!("  Before:       {mem_before:.2} MB");
    println!("  After spawn:  {mem_after_spawn:.2} MB");
    println!("  After asks:   {mem_after_ask:.2} MB");
    println!("  Final:        {mem_final:.2} MB");
    println!("  Per actor:    {:.2} KB", (mem_after_spawn - mem_before) * 1024.0 / n as f64);

    println!("\n{sep}\n");

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let counts: Vec<usize> = if args.len() > 1 {
        args[1..].iter().map(|a| a.parse().expect("invalid number")).collect()
    } else {
        vec![100, 1_000, 5_000, 10_000]
    };

    for n in counts {
        run_benchmark(n).await?;
    }

    Ok(())
}
