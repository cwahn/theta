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

    Ok(())
}
