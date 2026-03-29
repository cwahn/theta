//! Test fixture: a minimal host for resilience integration tests.
//!
//! Usage: resilience_host
//!   Prints PUBLIC_KEY:<key> to stdout, then serves actors until killed.

use std::hash::{Hash, Hasher};

use iroh::{Endpoint, dns::DnsResolver, endpoint::presets};
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

#[derive(Debug, Clone, ActorArgs)]
pub struct Counter {
    pub value: i64,
}

#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for Counter {
    type View = i64;

    const _: () = async |_: Ping| -> i64 {
        self.value += 1;
        self.value
    };

    const _: () = async |_: GetValue| -> i64 { self.value };

    fn hash_code(&self) -> u64 {
        let mut hasher = ahash::AHasher::default();
        self.value.hash(&mut hasher);
        hasher.finish()
    }
}

impl From<&Counter> for i64 {
    fn from(counter: &Counter) -> Self {
        counter.value
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn,theta=info")
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
    let counter = ctx.spawn(Counter { value: 0 });
    ctx.bind("counter", counter)?;

    println!("PUBLIC_KEY:{}", ctx.public_key());

    // Park until killed
    tokio::signal::ctrl_c().await.ok();
    Ok(())
}
