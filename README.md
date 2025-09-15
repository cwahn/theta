[![Crates.io](https://img.shields.io/crates/v/theta.svg)](https://crates.io/crates/theta)
[![Documentation](https://docs.rs/theta/badge.svg)](https://docs.rs/theta)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Theta

**An async actor framework for Rust**


<div align="center">
  <p>Any questions or idea?</p>
  <a href="https://github.com/cwahn/theta/discussions">
    <img alt="Join GitHub Discussions"
         src="https://img.shields.io/static/v1?label=&message=Join%20GitHub%20Discussions&color=0969da&style=flat&logo=github&logoColor=white"
         style="user-select: none; -webkit-user-select: none; -moz-user-select: none; -ms-user-select: none;">
  </a>
</div>


## Overview
Theta is an **ergonomic** yet **minimal** and **performant** async actor framework which Rust deserves.

- **Async**
  - An actor instance is a very thin wrapper around a `tokio::task` and two MPMC channels.
  - `ActorRef` is just a MPMC sender.
- **Built-in remote**
  - Distributed actor system powered by P2P network, `iroh`.
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
pub struct Inc(i64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for Counter {
    type View = Nil;

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
- Core
  - [x] Make `Result::Err` implementing `std::fmt::Display` on `tell` to be logged as `log::error!` to prevent silent failure or code duplication
  - [ ] Deduplicate simultanious connection attempt to each other
  - [ ] Use concurrent hashmap
- Macros
  - [ ] Make `actor` macro to take identifier as `ActorId`
- Supervision
  - [ ] Factor out supervision as a optional feature
- Remote
  - [ ] Define lifetime behavior of exported actors (Currently, exported actor will never get dropped)
  - [ ] Support full NodeAddr including Url format definition and sharing routing information between peers
  - [ ] Network timeout constants
- Persistence
  - [ ] Cover patiral persistance case; some could be stored in storage, but some data should be passed in runtime
  - [ ] Have respawn API to take closure, not value.
- Actor pool
  - [ ] Actor pool (task stealing with anonymous dynamic actors and MPMC)

## License

Licensed under the [MIT License](LICENSE).