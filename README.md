# ϑ theta

[![Crates.io](https://img.shields.io/crates/v/theta.svg)](https://crates.io/crates/theta)
[![Documentation](https://docs.rs/theta/badge.svg)](https://docs.rs/theta)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**An async actor framework for Rust**

Theta provides an actor model implementation, designed for building resilient and high performance concurrent systems.

## Design Goal
> **Ergonomic & performant async actor framework, which Rust deserves.**

## Features
- **Async** - Inherently asynchronous, built on top of Tokio
- **Built-in remote** - P2P distributed actor system powered by `iroh`. Even `ActorRef` could be passed around network boundary as regular data in message.
- **Built-in monitoring** - Carl Hewitt's Actor Model suggests concept of monitor, implemented as (possibly remote) monitoring feature.
- **Built-in persistence** - Seamless respawn of actor from snapshot on file system, AWS S3 etc.
- **WASM support** - Compile to WebAssembly for running in browser or other WASM environments

## Installation

```sh
cargo add theta
```

## 🚧 WIP
Theta is currently under active development and API is subject to change. Not recommended for production use yet.

## License

Licensed under the [MIT License](LICENSE).