use std::str::FromStr;

use iroh::PublicKey;
use theta::prelude::GlobalContext;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f %Z".into()))
        .pretty()
        // .compact()
        .init();

    info!("Initializing GlobalContext...");

    let g_ctx = GlobalContext::initialize().await;
    let public_key = g_ctx.public_key();

    info!("GlobalContext initialized with public key: {public_key}");

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

    g_ctx.connect_peer(other_public_key).await?;

    Ok(())
}
