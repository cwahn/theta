use std::{str::FromStr, time::Instant, vec};

use iroh::{Endpoint, PublicKey, dns::DnsResolver};
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;
// use theta_macros::{ActorConfig, impl_id};
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
            info!("Received ping from {}", msg.source);
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

    info!("Spawning PingPong actor...");
    let ping_pong = ctx.spawn(PingPong);

    info!("Binding PingPong actor to 'ping_pong' name...");
    ctx.bind(b"ping_pong", ping_pong);

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
            Ok(key) => break key,
        };
    };

    let ping_pong_url = Url::parse(&format!("iroh://ping_pong@{other_public_key}"))?;

    let other_ping_pong = match ActorRef::<PingPong>::lookup(&ping_pong_url).await {
        Err(e) => {
            error!("Failed to find PingPong actor at URL: {ping_pong_url}. Error: {e}");
            return Ok(());
        }
        Ok(actor) => actor,
    };

    info!("Sending ping to {ping_pong_url} every 5 seconds. Press Ctrl-C to stop.",);

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

    loop {
        interval.tick().await;

        let ping = Ping {
            source: public_key.clone(),
        };

        info!("Sending ping to {}", other_ping_pong.id());
        let sent_instant = Instant::now();
        match other_ping_pong.ask(ping).await {
            Err(e) => error!("Failed to send ping: {e}"),
            Ok(_pong) => {
                let elapsed = sent_instant.elapsed();
                info!("Received pong from {} in {elapsed:?}", other_ping_pong.id());
            }
        }
    }
}
