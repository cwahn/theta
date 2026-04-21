use std::{str::FromStr, time::Duration};

use counter::{Counter, Dec, GetWorker, Inc, Manager};
use crossterm::{
    event::{Event, KeyCode, KeyModifiers, read},
    terminal,
};
use iroh::{PublicKey, dns::DnsResolver};
use theta::{monitor::Update, prelude::*};
use theta_flume::unbounded_anonymous;
use tracing::{error, info};
use tracing_subscriber::fmt::time::ChronoLocal;
use url::Url;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%H:%M:%S".into()))
        .init();
    tracing_log::LogTracer::init().ok();

    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .dns_resolver(dns)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;
    let _ctx = RootContext::init(endpoint);

    info!("please enter the public key of the other peer:");

    let host_pk = tokio::task::spawn_blocking(|| {
        let mut input = String::new();

        loop {
            std::io::stdin()
                .read_line(&mut input)
                .expect("failed to read stdin");

            let trimmed = input.trim();

            if trimmed.is_empty() {
                error!("public key cannot be empty. Please try again.");

                input.clear();

                continue;
            }

            match PublicKey::from_str(trimmed) {
                Err(e) => {
                    error!("invalid public key format: {e}");

                    input.clear();
                }
                Ok(public_key) => break public_key,
            };
        }
    })
    .await?;
    let url = Url::parse(&format!("iroh://manager@{host_pk}"))?;

    info!("looking up Manager actor {url}");

    let manager = ActorRef::<Manager>::lookup(&url).await?;
    let worker_via_ask: ActorRef<Counter> = manager
        .ask(GetWorker)
        .timeout(Duration::from_secs(5))
        .await?;

    info!("monitoring manager actor {url}");

    let (tx, rx) = unbounded_anonymous();

    if let Err(e) = monitor::<Manager>(url, tx).await {
        error!("failed to monitor worker: {e}");
    }

    let Some(init_update) = rx.recv().await else {
        panic!("failed to receive initial update");
    };

    let Update::State(Manager {
        worker: worker_via_update,
    }) = init_update
    else {
        panic!("unexpected update type: {init_update:?}");
    };

    if worker_via_ask.id() == worker_via_update.id() {
        info!("worker actor IDs match: {}", worker_via_ask.id());
    } else {
        error!(
            "worker actor IDs do not match: ask: {} != update: {}",
            worker_via_ask.id(),
            worker_via_update.id()
        );
    }

    let counter_url = Url::parse(&format!("iroh://{}@{host_pk}", worker_via_ask.id()))?;
    let (counter_tx, counter_obs) = unbounded_anonymous();

    info!("monitoring counter actor {counter_url}");

    if let Err(e) = monitor::<Counter>(counter_url, counter_tx).await {
        error!("failed to monitor Counter actor: {e}");

        return Err(e.into());
    }

    tokio::spawn(async move {
        while let Some(value) = counter_obs.recv().await {
            info!("counter = {value:?}\r");
        }
    });

    info!("press ↑ / ↓ to Inc/Dec; Ctrl-C to quit.");

    terminal::enable_raw_mode()?;

    let result = loop {
        match read()? {
            Event::Key(k) if k.code == KeyCode::Up => {
                let _ = worker_via_ask
                    .ask(Inc)
                    .timeout(Duration::from_secs(5))
                    .await;
            }
            Event::Key(k) if k.code == KeyCode::Down => {
                let _ = worker_via_ask
                    .ask(Dec)
                    .timeout(Duration::from_secs(5))
                    .await;
            }
            Event::Key(k)
                if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                info!("Ctrl+C pressed, exiting...");

                break Ok(());
            }
            Event::Key(k) if k.code == KeyCode::Esc => {
                info!("Escape pressed, exiting...");

                break Ok(());
            }
            _ => {}
        }
    };

    terminal::disable_raw_mode()?;

    result
}
