use std::hash::{Hash, Hasher};

use iroh::Endpoint;
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;
use tracing::{debug, error, info};
use tracing_subscriber::fmt::time::ChronoLocal;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inc;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dec;

#[derive(Debug, Clone, ActorArgs)]
pub struct Counter {
    pub value: i64,
}

#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for Counter {
    type StateReport = i64;

    const _: () = {
        async |_: Inc| -> i64 {
            self.value += 1;
            self.value
        };

        async |_: Dec| -> i64 {
            self.value -= 1;
            self.value
        };
    };

    fn hash_code(&self) -> u64 {
        let mut hasher = rustc_hash::FxHasher::default();
        self.value.hash(&mut hasher);
        hasher.finish()
    }
}

impl From<&Counter> for i64 {
    fn from(counter: &Counter) -> Self {
        counter.value
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorker;

#[derive(Debug, Clone, ActorArgs, Serialize, Deserialize)]
pub struct Manager {
    pub worker: ActorRef<Counter>,
}

#[actor("f65b84e6-adfe-4d3a-8140-ee55de512070")]
impl Actor for Manager {
    type StateReport = Self;

    const _: () = {
        // expose the worker via ask
        async |_: GetWorker| -> ActorRef<Counter> { self.worker.clone() };
    };
}

impl From<&Manager> for Manager {
    fn from(manager: &Manager) -> Self {
        manager.clone()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%H:%M:%S".into()))
        .init();

    let endpoint = Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);

    // spawn worker counter (start at 0) and manager that wraps it
    let worker = ctx.spawn(Counter { value: 0 });
    debug!("Spawned worker actor: {}", worker.id());
    let manager = ctx.spawn(Manager { worker: worker });
    debug!("Spawned manager actor: {}", manager.id());

    // bind a discoverable name for the manager
    info!("Binding manager actor to 'manager' name...");
    ctx.bind(b"manager", manager);

    println!("host ready. public key: {}", ctx.public_key());

    // park forever
    futures::future::pending::<()>().await;
    Ok(())
}
