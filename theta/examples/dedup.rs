use std::{env, process::Command, str::FromStr, time::Instant};

use iroh::{Endpoint, PublicKey, SecretKey, dns::DnsResolver};
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;
use tracing::{error, info};

use tracing_subscriber::fmt::time::ChronoLocal;
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
        async |msg: Ping| -> Pong {
            info!("received ping from {}", msg.source);
            Pong {}
        };
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber first, then LogTracer
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f %Z".into()))
        .init();

    tracing_log::LogTracer::init().ok();

    let args: Vec<String> = env::args().collect();

    // Parse command line arguments for keys
    let (our_secret_key, other_public_key, child_secret_for_spawn) = if args.len() == 3 {
        // Child process mode: program_name public_key secret_key
        // args[0] = program_name, args[1] = public_key, args[2] = secret_key
        info!("Child process mode - using provided keys");
        info!("Received args[1] (other's public key): {}", &args[1]);
        info!(
            "Received args[2] (our secret key bytes): {} chars",
            &args[2].len()
        );

        let other_public_key = PublicKey::from_str(&args[1])?;
        info!("Parsed other_public_key: {}", other_public_key);

        // Parse the debug format of bytes array: "[1, 2, 3, ...]"
        let bytes_str = args[2].trim_start_matches('[').trim_end_matches(']');
        let our_secret_key_bytes: Vec<u8> = bytes_str
            .split(", ")
            .map(|s| s.parse::<u8>())
            .collect::<Result<Vec<u8>, _>>()?;
        let our_secret_key = SecretKey::try_from(&our_secret_key_bytes[..])?;
        info!(
            "Parsed our_secret_key, public version: {}",
            our_secret_key.public()
        );

        (our_secret_key, other_public_key, None)
    } else {
        // Parent process mode: generate two secret keys
        info!("Parent process mode - generating keys...");

        let mut rng = thread_rng();
        let parent_secret = SecretKey::generate(&mut rng);
        let child_secret = SecretKey::generate(&mut rng);

        let parent_public = parent_secret.public();
        let child_public = child_secret.public();

        info!("Generated parent key: {}", parent_public);
        info!("Generated child key: {}", child_public);
        info!(
            "Parent will connect to child. Parent will pass child's public key: {}",
            child_public
        );
        info!(
            "Child will connect to parent. Child will receive parent's public key: {}",
            parent_public
        );

        (parent_secret, child_public, Some(child_secret))
    };

    // Spawn child process if we're in parent mode
    if let Some(child_secret) = child_secret_for_spawn {
        // Convert child secret key bytes to string for passing to child process
        let child_secret_bytes = child_secret.to_bytes();
        let child_secret_str = format!("{:?}", child_secret_bytes);

        info!("Spawning child process with:");
        info!(
            "  - arg[1] (parent's public key for child to connect to): {}",
            our_secret_key.public()
        );
        info!(
            "  - arg[2] (child's secret key): {} bytes",
            child_secret_bytes.len()
        );

        use std::fs::OpenOptions;
        use std::process::Stdio;

        // Create log file for child process with ANSI color support
        let child_log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open("child_process.log")?;

        let _child = Command::new("./target/debug/examples/dedup")
            .args(&[&our_secret_key.public().to_string(), &child_secret_str])
            .stdout(Stdio::from(child_log_file.try_clone()?))
            .stderr(Stdio::from(child_log_file))
            .env("FORCE_COLOR", "1") // Force ANSI colors even when redirecting
            .env("CLICOLOR_FORCE", "1") // Additional color forcing
            .spawn()?;

        info!("Child process spawned. Both will try to connect simultaneously.");
        info!("Child process logs are being written to: child_process.log");
    }

    // Common initialization code
    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .dns_resolver(dns)
        .secret_key(our_secret_key)
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();

    info!("RootContext initialized with public key: {public_key}");
    info!("Will attempt to connect to other peer with public key: {other_public_key}");
    info!(
        "Key comparison: our_key={}, other_key={}, our_key > other_key = {}",
        public_key,
        other_public_key,
        public_key > other_public_key
    );

    info!("spawning PingPong actor...");
    let ping_pong = ctx.spawn(PingPong);

    info!("binding PingPong actor to 'ping_pong' name...");
    let _ = ctx.bind("ping_pong", ping_pong);

    // Give some time for the actor to become fully discoverable and network to stabilize
    // tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

    // Implement connection tie-breaking: only the peer with higher public key initiates
    // let should_initiate = public_key > other_public_key;

    // if should_initiate {
    //     info!("This peer ({}) has higher public key, will initiate connection to {}", public_key, other_public_key);
    // } else {
    //     info!("This peer ({}) has lower public key, will wait for connection from {}", public_key, other_public_key);
    //     // Add extra delay for the peer that should wait
    //     tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;
    // }

    // Common connection and ping logic
    info!("Connecting to peer: {}", other_public_key);

    let ping_pong_url = Url::parse(&format!("iroh://ping_pong@{other_public_key}"))?;
    info!("Constructed URL for lookup: {}", ping_pong_url);

    // Retry lookup with exponential backoff
    let mut retry_delay = tokio::time::Duration::from_millis(500);
    let max_retry_delay = tokio::time::Duration::from_secs(30);
    let mut retry_count = 0;

    let other_ping_pong = loop {
        info!(
            "Attempting lookup {} for {}",
            retry_count + 1,
            ping_pong_url
        );
        match ActorRef::<PingPong>::lookup(&ping_pong_url).await {
            Ok(actor) => {
                info!(
                    "Successfully connected to peer after {} attempts",
                    retry_count + 1
                );
                break actor;
            }
            Err(e) => {
                retry_count += 1;
                error!(
                    "Lookup attempt {} failed for URL: {ping_pong_url}. Error: {e}",
                    retry_count
                );

                // Wait before retrying with exponential backoff
                info!("Retrying in {:?}...", retry_delay);
                tokio::time::sleep(retry_delay).await;

                // Increase delay for next retry, capped at max_retry_delay
                retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
            }
        }
    };

    info!("sending ping to {ping_pong_url} every 5 seconds. Press Ctrl-C to stop.");

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

    // Setup shutdown signal handling
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Received Ctrl-C signal, shutting down...");
        let _ = shutdown_tx.send(true);
    });

    let mut current_actor_ref = other_ping_pong;

    loop {
        // Check for shutdown signal
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received, exiting...");
                    break;
                }
            }
            _ = interval.tick() => {
                // Continue with ping logic
            }
        }

        let ping = Ping {
            source: public_key.clone(),
        };

        info!("sending ping to {}", current_actor_ref.id());
        let sent_instant = Instant::now();
        match current_actor_ref.ask(ping).await {
            Err(e) => {
                error!("failed to send ping: {e}");
                info!("Connection lost, attempting to reconnect...");

                // Try to reconnect
                let mut reconnect_delay = tokio::time::Duration::from_millis(500);
                let max_reconnect_delay = tokio::time::Duration::from_secs(10);
                let mut reconnect_attempts = 0;

                loop {
                    match ActorRef::<PingPong>::lookup(&ping_pong_url).await {
                        Ok(actor) => {
                            info!(
                                "Successfully reconnected after {} attempts",
                                reconnect_attempts + 1
                            );
                            current_actor_ref = actor;
                            break;
                        }
                        Err(e) => {
                            reconnect_attempts += 1;
                            error!("Reconnection attempt {} failed: {e}", reconnect_attempts);

                            info!("Retrying reconnection in {:?}...", reconnect_delay);
                            tokio::time::sleep(reconnect_delay).await;

                            // Increase delay for next retry, capped at max_reconnect_delay
                            reconnect_delay =
                                std::cmp::min(reconnect_delay * 2, max_reconnect_delay);
                        }
                    }
                }
            }
            Ok(_pong) => {
                let elapsed = sent_instant.elapsed();
                info!(
                    "received pong from {} in {elapsed:?}",
                    current_actor_ref.id()
                );
            }
        }
    }

    Ok(())
}
