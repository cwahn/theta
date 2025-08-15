use std::{
    hash::{Hash, Hasher},
    str::FromStr,
};

use crossterm::{
    event::{Event, KeyCode, read},
    terminal,
};
use iroh::PublicKey;
use serde::{Deserialize, Serialize};
use theta::{monitor::Report, prelude::*};
use theta_flume::unbounded_anonymous;
use theta_macros::ActorArgs;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

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
        .with_env_filter("info,theta=info")
        .with_timer(ChronoLocal::new("%H:%M:%S".into()))
        .compact()
        .init();

    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![b"theta".to_vec()])
        .discovery_n0()
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);

    // 1) get host pubkey
    info!("Please enter the public key of the other peer:");
    let mut input = String::new();
    let host_pk = loop {
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            error!("Public key cannot be empty. Please try again.");
            input.clear();
            continue;
        }
        match PublicKey::from_str(trimmed) {
            Ok(key) => break key,
            Err(e) => {
                error!("Invalid public key format: {e}");
                input.clear();
            }
        };
    };

    // 2) lookup manager by name@host
    info!("Looking up manager actor at iroh://manager@{host_pk}...");
    let url = Url::parse(&format!("iroh://manager@{host_pk}"))?;
    let manager = ctx.lookup::<Manager>(&url).await?;

    // --- A) via ask ---
    let worker_via_ask: ActorRef<Counter> = manager.ask(GetWorker).await?;

    // --- B) via observe (state report) ---
    info!("Observing manager actor at iroh://manager@{host_pk}...");
    let (tx, rx) = unbounded_anonymous();
    if let Err(e) = observe::<Manager>(url, tx).await {
        error!("Failed to observe worker: {e}");
    }

    let Some(init_report) = rx.recv().await else {
        panic!("failed to receive initial report");
    };

    let Report::State(Manager {
        worker: worker_via_report,
    }) = init_report
    else {
        panic!("unexpected report type: {init_report:?}");
    };

    // verify same actor
    // ? duplicated import?
    // assert_eq!(worker_via_ask.id(), worker_via_report.id());
    // info!("same worker OK: {}", worker_via_ask.id());
    if worker_via_ask.id() == worker_via_report.id() {
        info!("Worker actor IDs match: {}", worker_via_ask.id());
    } else {
        error!(
            "Worker actor IDs do not match: ask: {} != report: {}",
            worker_via_ask.id(),
            worker_via_report.id()
        );
    }

    // subscribe to worker state updates
    let counter_url = Url::parse(&format!("iroh://{}@{host_pk}", worker_via_ask.id()))?;
    let (counter_tx, counter_obs) = unbounded_anonymous();

    if let Err(e) = observe::<Counter>(counter_url, counter_tx).await {
        // ! It should have actor id to abserve
        error!("Failed to observe Counter actor: {e}");
        return Err(e.into());
    }

    tokio::spawn(async move {
        while let Some(value) = counter_obs.recv().await {
            info!("counter = {value:?}");
        }
    });

    // simple key loop: ↑ -> Inc, ↓ -> Dec
    terminal::enable_raw_mode()?;
    info!("press ↑ / ↓ to Inc/Dec; Ctrl-C to quit.");
    loop {
        match read()? {
            Event::Key(k) if k.code == KeyCode::Up => {
                let _ = worker_via_ask.ask(Inc).await;
            }
            Event::Key(k) if k.code == KeyCode::Down => {
                let _ = worker_via_ask.ask(Dec).await;
            }
            _ => {}
        }
    }
}
