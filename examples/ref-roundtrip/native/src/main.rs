use iroh::{Endpoint, endpoint::presets};
use ref_actors::{Depot, SpawnItem};
use theta::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .compact()
        .init();

    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;
    let ctx = RootContext::init(endpoint);
    let my_key = ctx.public_key();

    ctx.endpoint().online().await;

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
    println!("PEER_KEY:{my_key}");
    tokio::signal::ctrl_c().await?;

    Ok(())
}
