use std::{str::FromStr, time::Instant};

use iroh::{Endpoint, PublicKey, dns::DnsResolver};
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

#[derive(Debug, Clone, ActorArgs)]
pub struct ForwardPingPong;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping {
    pub source: PublicKey,
    pub target: ActorRef<ForwardPingPong>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pong {}

#[actor("a1b2c3d4-5e6f-7890-1234-567890abcdef")]
impl Actor for ForwardPingPong {
    const _: () = {
        async |msg: Ping| -> Pong {
            info!(
                "Received forwarded ping from {}, waiting 0.75 seconds...",
                msg.source
            );

            // Wait 0.75 seconds before responding with Pong
            tokio::time::sleep(tokio::time::Duration::from_millis(750)).await;

            info!("Sending pong back to {}", msg.source);
            Pong {}
        };
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f %Z".into()))
        .init();

    tracing_log::LogTracer::init().ok();

    info!("Initializing RootContext...");

    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .dns_resolver(dns) // Required for mobile hotspot support
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();

    info!("RootContext initialized with public key: {public_key}");

    info!("Spawning ForwardPingPong actor...");
    let forward_ping_pong = ctx.spawn(ForwardPingPong);

    info!("Binding ForwardPingPong actor to 'forward-ping-pong' name...");
    ctx.bind(b"forward-ping-pong", forward_ping_pong.clone());

    // Ask for user of other peer's public key
    info!("Please enter the public key of the other peer:");

    let mut input = String::new();
    let other_public_key = loop {
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            error!("Public key cannot be empty. Please try again.");
            input.clear();
            continue;
        }

        match PublicKey::from_str(trimmed) {
            Err(e) => {
                error!("Invalid public key format: {e}");
                input.clear();
            }
            Ok(public_key) => break public_key,
        };
    };

    let forward_ping_pong_url =
        Url::parse(&format!("iroh://forward-ping-pong@{other_public_key}"))?;

    let other_forward_ping_pong = match ActorRef::<ForwardPingPong>::lookup(&forward_ping_pong_url)
        .await
    {
        Err(e) => {
            error!(
                "Failed to find ForwardPingPong actor at URL: {forward_ping_pong_url}. Error: {e}"
            );
            return Ok(());
        }
        Ok(actor) => actor,
    };

    info!("Starting forward ping cycle every 2 seconds. Press Ctrl-C to stop.");

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));

    loop {
        interval.tick().await;

        // Create a ping with self as target to forward to the other peer
        let ping = Ping {
            source: public_key.clone(),
            target: forward_ping_pong.clone(),
        };

        info!(
            "Forwarding ping to {} with self as target",
            other_forward_ping_pong.id()
        );
        let sent_instant = Instant::now();

        match other_forward_ping_pong.ask(ping).await {
            Err(e) => error!("Failed to forward ping: {e}"),
            Ok(_pong) => {
                let elapsed = sent_instant.elapsed();
                info!(
                    "Received pong from {} in {elapsed:?}",
                    other_forward_ping_pong.id()
                );
            }
        }
    }
}
