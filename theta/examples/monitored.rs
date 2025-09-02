use iroh::Endpoint;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};
use std::{
    hash::{Hash, Hasher},
    vec,
};
use theta::prelude::*;
use theta_macros::ActorArgs;
use tracing::{error, info};

use tracing_subscriber::fmt::time::ChronoLocal;

#[derive(Debug, Clone, Hash, ActorArgs, Serialize, Deserialize)]
pub struct Counter {
    value: i64,
}

impl Counter {
    pub fn new() -> Self {
        Self { value: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inc {
    pub amount: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dec {
    pub amount: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterResponse {
    pub new_value: i64,
}

#[actor("a1b2c3d4-5e6f-7890-abcd-ef1234567890")]
impl Actor for Counter {
    type View = Counter;

    const _: () = {
        async |msg: Inc| -> CounterResponse {
            let new_value = self.value + msg.amount;
            self.value = new_value;
            info!("Counter incremented by {} to {}", msg.amount, new_value);
            CounterResponse { new_value }
        };
    };

    const _: () = {
        async |msg: Dec| -> CounterResponse {
            let new_value = self.value - msg.amount;
            self.value = new_value;
            info!("Counter decremented by {} to {}", msg.amount, new_value);
            CounterResponse { new_value }
        };
    };

    fn hash_code(&self) -> u64 {
        let mut hasher = FxHasher::default();
        Hash::hash(self, &mut hasher);
        hasher.finish()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_log::LogTracer::init().ok();
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%y%m%d %H:%M:%S%.3f %Z".into()))
        .init();

    info!("Initializing RootContext...");
    let endpoint = Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();
    info!("RootContext initialized with public key: {public_key}");

    info!("Spawning Counter actor...");
    let counter = ctx.spawn(Counter::new());

    info!("Binding Counter actor to 'counter' name...");
    ctx.bind(b"counter", counter.clone());

    info!("Counter actor is now running and bound to 'counter'");
    info!("Public key: {public_key}");
    info!("Ready to receive Inc/Dec messages. Press Ctrl-C to stop.");

    // Keep the application running
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        if let Err(e) = counter.tell(Inc { amount: 1 }) {
            error!("Failed to send Inc message: {e}");
            return Err(e.into());
        }
    }
}
