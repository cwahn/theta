use iroh::{Endpoint, dns::DnsResolver, endpoint::presets};
use monitoring::{Counter, Inc};
use theta::prelude::*;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;

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

    info!("spawning Counter actor...");

    let counter = ctx.spawn(Counter::new());

    info!("binding Counter actor to 'counter' name...");

    let _ = ctx.bind("counter", counter.clone());

    info!("counter actor is now running and bound to 'counter'");

    info!("public key: {public_key}");

    info!("ready to receive Inc/Dec messages. Press Ctrl-C to stop.");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let Err(e) = counter.tell(Inc { amount: 1 }) else {
            continue;
        };

        error!("failed to send Inc message: {e}");

        return Err(e.into());
    }
}
