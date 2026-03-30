use std::str::FromStr;

use iroh::{Endpoint, PublicKey, dns::DnsResolver, endpoint::presets};
use monitoring::Counter;
use theta::{monitor::monitor, prelude::*};
use theta_flume::unbounded_anonymous;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%y%m%d %H:%M:%S%.3f %Z".into()))
        .init();

    tracing_log::LogTracer::init().ok();

    info!("initializing RootContext...");
    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder(presets::N0)
        .dns_resolver(dns)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();
    info!("rootContext initialized with public key: {public_key}");

    println!("Please enter the public key of the other peer:");
    let other_public_key = tokio::task::spawn_blocking(|| {
        let mut input = String::new();
        loop {
            std::io::stdin()
                .read_line(&mut input)
                .expect("failed to read stdin");
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
        }
    })
    .await?;

    let counter_url = Url::parse(&format!("iroh://counter@{other_public_key}"))?;
    let (tx, rx) = unbounded_anonymous();

    if let Err(e) = monitor::<Counter>(counter_url, tx).await {
        error!("failed to monitor Counter actor: {e}");
        return Err(e.into());
    }

    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            info!("received state update: {update:#?}");
        }
    });

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}
