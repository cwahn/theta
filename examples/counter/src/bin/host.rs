use counter::{Counter, Manager};
use iroh::{Endpoint, dns::DnsResolver, endpoint::presets};
use theta::prelude::*;
use tracing::info;
use tracing_subscriber::fmt::time::ChronoLocal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%H:%M:%S".into()))
        .compact()
        .init();
    tracing_log::LogTracer::init().ok();

    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder(presets::N0)
        .dns_resolver(dns)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;
    let ctx = RootContext::init(endpoint);
    let worker = ctx.spawn(Counter { value: 0 });
    let manager = ctx.spawn(Manager { worker });

    info!("binding manager actor to 'manager' name...");

    let _ = ctx.bind("manager", manager);

    println!("host ready. public key: {}", ctx.public_key());
    futures::future::pending::<()>().await;

    Ok(())
}
