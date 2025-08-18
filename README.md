# 🔗 Theta

[![Crates.io](https://img.shields.io/crates/v/theta.svg)](https://crates.io/crates/theta)
[![Documentation](https://docs.rs/theta/badge.svg)](https://docs.rs/theta)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**An async actor framework for Rust**

## Overview
Theta is an **ergonomic** yet **minimal** and **performant** async actor framework which Rust deserves.

- **Async**
  - An actor instance is a very thin wrapper around a `tokio::task` and two MPSC channels.
  - `ActorRef` is just a MPSC sender.
- **Built-in remote**
  - Distributed actor system powered by P2P protocol, `iroh`.
  - Even `ActorRef` could be passed around network boundary as regular data in message.
  - Available with feature `remote`.
- **Built-in monitoring**
  - "Monitor" suggested by Carl Hewitt's Actor Model is implemented as (possibly remote) monitoring feature.
  - Available with feature `monitor`.
- **Built-in persistence**
  - Seamless respawn of actor from snapshot on file system, AWS S3 etc.
  - Available with feature `persistence`.
- **WASM support (WIP)**
  - Compile to WebAssembly for running in browser or other WASM environments

## Example
```sh
cargo add theta
```

```rust
use serde::{Deserialize, Serialize};
use theta::prelude::*;

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
    type StateReport = Nil;

    // Behaviors will generate single enum Msg for the actor
    const _: () = {
        async |Inc(amount): Inc| { // Behavior can access &mut self
            self.value += amount;
        };

        async |_: GetValue| -> i64 {  // Behavior may or may not have return
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
```


## 🚧 WIP
Theta is currently under active development and API is subject to change. Not yet recommended for any serious business.
### Todo
- [ ] Factor out supervision as a optional feature
- [ ] Define lifetime behavior of exported actors (Currently, exported actor will never get dropped)

## License

Licensed under the [MIT License](LICENSE).