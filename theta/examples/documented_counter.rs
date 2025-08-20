//! Comprehensive example demonstrating theta's core features with documentation.
//!
//! This example shows:
//! - Basic actor definition and usage
//! - Message handling with the #[actor] macro  
//! - Fire-and-forget vs request-response messaging
//! - State monitoring (with monitor feature)

use serde::{Deserialize, Serialize};
use theta::prelude::*;

/// Actor state for a simple counter
#[derive(Debug, Clone, ActorArgs)]
struct Counter {
    value: i64,
}

/// Message to increment the counter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inc(i64);

/// Message to get the current counter value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

/// Message to reset the counter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reset;

/// Counter actor implementation
#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for Counter {
    /// We don't need state updateing for this simple example
    type View = Nil;

    // Define message handlers using the macro syntax
    const _: () = {
        // Fire-and-forget message handler
        async |Inc(amount): Inc| {
            self.value += amount;
        };

        // Request-response message handler
        async |_: GetValue| -> i64 { self.value };

        // Another fire-and-forget handler
        async |_: Reset| {
            self.value = 0;
        };
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize the actor system
    let ctx = RootContext::init_local();

    // Spawn a counter actor
    let counter = ctx.spawn(Counter { value: 0 });

    // Demonstrate fire-and-forget messaging
    println!("=== Fire-and-forget messaging ===");
    counter.tell(Inc(5))?;
    counter.tell(Inc(3))?;

    // Small delay to let messages process
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Demonstrate request-response messaging
    println!("\n=== Request-response messaging ===");
    let current_value = counter.ask(GetValue).await?;
    println!("Current counter value: {}", current_value);

    // Reset and check again
    counter.tell(Reset)?;
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let reset_value = counter.ask(GetValue).await?;
    println!("Value after reset: {}", reset_value);

    // Demonstrate actor binding for lookup
    println!("\n=== Actor binding and lookup ===");
    ctx.bind(b"main_counter", counter.clone());

    // Look up the bound actor (for remote feature, lookup takes string/URL)
    #[cfg(feature = "remote")]
    {
        if let Ok(found_counter) = ctx.lookup::<Counter>("main_counter").await {
            found_counter.tell(Inc(42))?;
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            let final_value = found_counter.ask(GetValue).await?;
            println!("Final value via lookup: {}", final_value);
        }
    }

    #[cfg(not(feature = "remote"))]
    {
        // For local-only, just use the bound actor directly
        counter.tell(Inc(42))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let final_value = counter.ask(GetValue).await?;
        println!("Final value: {}", final_value);
    }

    #[cfg(feature = "monitor")]
    {
        use theta_flume::unbounded_anonymous;

        println!("\n=== State monitoring ===");

        // Create a channel for receiving state updates
        let (tx, rx) = unbounded_anonymous();

        // Start observing the counter (by name)
        if let Err(e) = monitor::<Counter>("main_counter", tx).await {
            println!("Failed to monitor counter: {}", e);
        } else {
            // Make some changes to trigger state updates
            counter.tell(Inc(10))?;
            counter.tell(Inc(-5))?;

            // Receive a few state updates
            for _ in 0..3 {
                if let Ok(update) =
                    tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await
                {
                    if let Some(update) = update {
                        println!("Received state update: {:?}", update);
                    }
                } else {
                    break;
                }
            }
        }
    }

    println!("\n=== Example completed ===");

    Ok(())
}
