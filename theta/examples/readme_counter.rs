use serde::{Deserialize, Serialize};
use theta::prelude::*;
use tracing::info;

#[derive(Debug, Clone, ActorArgs)]
struct Counter {
    value: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Inc(i64);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GetValue;

#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for Counter {
    type View = Nil;

    // Behaviors will generate single enum Msg for the actor
    const _: () = {
        async |Inc(amount): Inc| {
            // Behavior can access &mut self
            self.value += amount;
        };

        async |_: GetValue| -> i64 {
            // Behavior may or may not have return
            self.value
        };
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let ctx = RootContext::init_local();
    let counter = ctx.spawn(Counter { value: 0 });

    let _ = counter.tell(Inc(5)); // Fire-and-forget

    let current = counter.ask(GetValue).await?; // Wait for response
    println!("Current value: {current}"); // Current value: 5

    Ok(())
}
