/// Minimal reproduction: isolate QUIC bi-stream opening from the actor framework.
/// Tests concurrent vs batched open_bi at various N to find failure threshold.
///
/// Mode 1 (default): In-process test — both endpoints in same process.
/// Mode 2 (--server): Run as server, print public key for client.
/// Mode 3 (<PUBLIC_KEY>): Run as client connecting to external server.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{presets, QuicTransportConfig, VarInt};
use iroh::Endpoint;
use tokio::sync::Semaphore;

async fn run_accept_loop(endpoint: Endpoint) {
    let conn = endpoint.accept().await.unwrap().await.unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted2 = accepted.clone();

    // Print progress in background
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("  [server] accepted: {}", accepted2.load(Ordering::Relaxed));
        }
    });

    loop {
        match conn.accept_bi().await {
            Ok((_send, mut recv)) => {
                accepted.fetch_add(1, Ordering::Relaxed);
                // Read the tiny payload, then drop the streams
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 64];
                    let _ = recv.read(&mut buf).await;
                });
            }
            Err(e) => {
                eprintln!("  [server] accept_bi error: {e:?}");
                break;
            }
        }
    }
}

async fn test_concurrent_open(conn: &iroh::endpoint::Connection, n: usize) -> (usize, usize, Duration) {
    let t = Instant::now();
    let ok = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let conn = conn.clone();
        let ok = ok.clone();
        let fail = fail.clone();
        handles.push(tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(30), conn.open_bi()).await {
                Ok(Ok((mut send, _recv))) => {
                    // Send a tiny payload
                    match send.write_all(b"hi").await {
                        Ok(_) => { ok.fetch_add(1, Ordering::Relaxed); }
                        Err(_) => { fail.fetch_add(1, Ordering::Relaxed); }
                    }
                }
                Ok(Err(e)) => {
                    if fail.load(Ordering::Relaxed) == 0 {
                        eprintln!("  first open_bi error: {e:?}");
                    }
                    fail.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    fail.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let dur = t.elapsed();
    (ok.load(Ordering::Relaxed), fail.load(Ordering::Relaxed), dur)
}

async fn test_batched_open(
    conn: &iroh::endpoint::Connection,
    n: usize,
    batch_size: usize,
) -> (usize, usize, Duration) {
    let t = Instant::now();
    let semaphore = Arc::new(Semaphore::new(batch_size));
    let ok = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let conn = conn.clone();
        let sem = semaphore.clone();
        let ok = ok.clone();
        let fail = fail.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            match tokio::time::timeout(Duration::from_secs(30), conn.open_bi()).await {
                Ok(Ok((mut send, _recv))) => {
                    match send.write_all(b"hi").await {
                        Ok(_) => { ok.fetch_add(1, Ordering::Relaxed); }
                        Err(_) => { fail.fetch_add(1, Ordering::Relaxed); }
                    }
                }
                Ok(Err(e)) => {
                    if fail.load(Ordering::Relaxed) == 0 {
                        eprintln!("  first open_bi error (batched): {e:?}");
                    }
                    fail.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    fail.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let dur = t.elapsed();
    (ok.load(Ordering::Relaxed), fail.load(Ordering::Relaxed), dur)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let max_streams: u32 = std::env::var("MAX_STREAMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_100_000);

    let transport_config = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(VarInt::from_u32(max_streams))
        .max_concurrent_bidi_streams(VarInt::from_u32(max_streams))
        .build();

    // Server endpoint
    let server_ep = Endpoint::builder(presets::N0)
        .transport_config(transport_config.clone())
        .alpns(vec![b"stress".to_vec()])
        .bind()
        .await?;
    let server_addr = server_ep.addr();

    // Client endpoint
    let client_ep = Endpoint::builder(presets::N0)
        .transport_config(transport_config)
        .alpns(vec![b"stress".to_vec()])
        .bind()
        .await?;

    // Start server accept loop
    tokio::spawn(run_accept_loop(server_ep));

    // Connect client to server
    println!("Connecting to server...");
    let conn = client_ep.connect(server_addr.clone(), b"stress").await?;
    println!("Connected!\n");

    let sep = "=".repeat(60);
    println!("{sep}");
    println!("  QUIC Bi-Stream Stress Test");
    println!("{sep}");

    // Test 1: Concurrent open_bi at increasing N
    println!("\n--- Test 1: Unbounded concurrent open_bi ---");
    for &n in &[1000, 5000, 10_000, 50_000, 100_000, 500_000, 1_000_000] {
        print!("  N={n:>9}: ");
        let (ok, fail, dur) = test_concurrent_open(&conn, n).await;
        println!("{ok}/{n} OK, {fail} fail, {dur:?}");
        if fail > 0 {
            println!("  *** CONNECTION FAILED at N={n} — stopping concurrent test ***");
            // Check if connection is still alive
            match conn.open_bi().await {
                Ok(_) => println!("  (connection still alive after failures)"),
                Err(e) => {
                    println!("  (connection DEAD: {e:?})");
                    println!("\n  Reconnecting...");
                    break;
                }
            }
        }
        // Small pause between tests
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Reconnect if needed
    let conn = match conn.open_bi().await {
        Ok(_) => conn,
        Err(_) => {
            println!("  Reconnecting for batched test...");
            client_ep.connect(server_addr.clone(), b"stress").await?
        }
    };

    // Test 2: Batched open_bi (semaphore-limited) at 1M
    println!("\n--- Test 2: Batched open_bi at 1M (semaphore-limited) ---");
    for &batch in &[100, 500, 1000, 5000, 10_000] {
        // Reconnect fresh for each batch size
        let fresh_conn = client_ep.connect(server_addr.clone(), b"stress").await?;
        print!("  batch={batch:>6}: ");
        let (ok, fail, dur) = test_batched_open(&fresh_conn, 1_000_000, batch).await;
        println!("{ok}/1000000 OK, {fail} fail, {dur:?}");
        if fail > 0 {
            match fresh_conn.open_bi().await {
                Ok(_) => println!("  (connection survived)"),
                Err(e) => println!("  (connection DEAD: {e:?})"),
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!("\n{sep}");
    println!("  Done");
    println!("{sep}");

    Ok(())
}
