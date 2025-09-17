use iroh::{Endpoint, PublicKey};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, vec};
use theta::{monitor::monitor, prelude::*};
use theta_flume::unbounded_anonymous;
use theta_macros::ActorArgs;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

#[derive(Debug, Clone, Hash, Serialize, Deserialize, ActorArgs)]
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
            info!("counter incremented by {} to {}", msg.amount, new_value);
            CounterResponse { new_value }
        };
    };

    const _: () = {
        async |msg: Dec| -> CounterResponse {
            let new_value = self.value - msg.amount;
            self.value = new_value;
            info!("counter decremented by {} to {}", msg.amount, new_value);
            CounterResponse { new_value }
        };
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%y%m%d %H:%M:%S%.3f %Z".into()))
        .init();

    tracing_log::LogTracer::init().ok();

    info!("initializing RootContext...");
    let endpoint = Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();
    info!("rootContext initialized with public key: {public_key}");

    println!("Please enter the public key of the other peer:");
    let mut input = String::new();
    let other_public_key = loop {
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            eprintln!("Public key cannot be empty. Please try again.");
            input.clear();
            continue;
        }
        match PublicKey::from_str(trimmed) {
            Err(e) => {
                eprintln!("Invalid public key format: {e}");
                input.clear();
            }
            Ok(public_key) => break public_key,
        };
    };

    let couter_url = Url::parse(&format!("iroh://counter@{other_public_key}"))?;
    let (tx, rx) = unbounded_anonymous();

    if let Err(e) = monitor::<Counter>(couter_url, tx).await {
        error!("failed to monitor Counter actor: {e}");
        return Err(e.into());
    }

    // Alternatively, if one has the ActorRef already (e.g., from a previous lookup),
    // let counter =
    //     ActorRef::<Counter>::lookup(&format!("iroh://counter@{other_public_key}")).await?;
    // let (tx, rx) = unbounded_anonymous();
    // if let Err(e) = counter.monitor(tx).await {
    //     return Err(e.into());
    // }

    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            info!("received state update: {update:#?}",);
        }
    });

    // Keep the application running
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}
