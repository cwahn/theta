use std::time::Duration;

use serde::{Deserialize, Serialize};
use theta::prelude::*;

const COUNTER_UUID: uuid::Uuid = uuid::uuid!("96d9901f-24fc-4d82-8eb8-023153d41074");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

#[actor(COUNTER_UUID)]
impl Actor for Counter {
    const _: () = async |amount: i64| {
        self.value += amount;
    };

    const _: () = async |_: GetValue| -> i64 { self.value };
}

#[derive(Debug, Clone, ActorArgs)]
struct Counter {
    value: i64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let ctx = RootContext::init_local();
    let counter = ctx.spawn(Counter { value: 0 });
    let _ = counter.tell(5);
    let current = counter
        .ask(GetValue)
        .timeout(Duration::from_secs(1))
        .await?;

    println!("Current value: {current}");

    Ok(())
}
