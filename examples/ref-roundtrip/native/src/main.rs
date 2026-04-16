use iroh::{Endpoint, endpoint::presets};
use ref_actors::{Depot, SpawnItem};
use theta::prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .compact()
        .init();

    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let my_key = ctx.public_key();

    // Wait until the endpoint has registered with a relay server so the
    // WASM/TS side can discover this peer via pkarr lookup.
    ctx.endpoint().online().await;

    // Spawn Depot and pre-seed it with three named items so that
    // GetBundle / GetGroup / GetNested handlers always have data.
    let depot = ctx.spawn(Depot::default());
    depot
        .ask(SpawnItem {
            label: "alpha".to_string(),
        })
        .await?;
    depot
        .ask(SpawnItem {
            label: "beta".to_string(),
        })
        .await?;
    depot
        .ask(SpawnItem {
            label: "gamma".to_string(),
        })
        .await?;

    ctx.bind("depot", depot).expect("bind failed");

    // Signal to the test driver that we are ready.
    println!("PEER_KEY:{my_key}");

    // Run until cancelled.
    tokio::signal::ctrl_c().await?;
    Ok(())
}
