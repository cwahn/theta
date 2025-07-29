use std::{pin::Pin, str::FromStr};

use iroh::PublicKey;
use serde::{Deserialize, Serialize};
use theta::{
    actor::Actor,
    prelude::{Behavior, Context, GlobalContext},
};
use theta_macros::{ActorConfig, impl_id};
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

#[derive(Debug, Clone, ActorConfig)]
struct PingPong {
    source: PublicKey,
}

#[impl_id("bd8d4895-9e5e-41d6-9f51-ec123a33e1c4")]
impl Actor for PingPong {}

#[derive(Debug, Serialize, Deserialize)]
struct Ping {
    pub source: PublicKey,
}

#[derive(Debug, Serialize, Deserialize)]
struct Pong {}

#[impl_id("f68fe56f-8aa9-4f90-8af8-591a06e2818a")]
impl Behavior<Ping> for PingPong {
    type Return = Pong;

    async fn process(&mut self, _ctx: Context<Self>, _msg: Ping) -> Self::Return {
        info!("Ping received from: {}", self.source);
        Pong {}
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f %Z".into()))
        // .pretty()
        // .compact()
        .init();

    info!("Initializing GlobalContext...");

    let g_ctx = GlobalContext::initialize().await;
    let public_key = g_ctx.public_key();

    info!("GlobalContext initialized with public key: {public_key}");

    info!("Spawning PingPong actor...");
    let ping_pong = g_ctx
        .spawn(PingPong {
            source: public_key.clone(),
        })
        .await;

    info!("Binding PingPong actor to 'ping_pong' name...");
    g_ctx.bind("ping_pong", ping_pong);

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
            Ok(key) => break key,
            Err(e) => {
                error!("Invalid public key format: {e}");
                input.clear();
            }
        };
    };

    // * confimrmed to work
    // g_ctx.connect_peer(other_public_key).await?;

    let ping_pong_url = Url::parse(&format!(
        "iroh://{}/ping_pong",
        other_public_key.to_string()
    ))?;

    let Some(other_ping_pong) = g_ctx.lookup::<PingPong>(&ping_pong_url).await else {
        error!("Failed to find PingPong actor at URL: {}", ping_pong_url);
        return Ok(());
    };

    // Send ping until ctrl-c comes in for every 5 seconds
    info!(
        "Sending ping to {} every 5 seconds. Press Ctrl-C to stop.",
        ping_pong_url
    );

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

    loop {
        interval.tick().await;

        let ping = Ping {
            source: public_key.clone(),
        };

        info!("Sending ping to {}", other_ping_pong.id());
        match other_ping_pong.ask(ping).await {
            Ok(_) => info!("Received pong successfully!"),
            Err(e) => error!("Failed to send ping: {}", e),
        }
    }

    Ok(())
}
