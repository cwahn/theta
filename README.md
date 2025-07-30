# Theta

[![Crates.io](https://img.shields.io/crates/v/theta.svg)](https://crates.io/crates/theta)
[![Documentation](https://docs.rs/theta/badge.svg)](https://docs.rs/theta)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**An async actor framework for Rust**

Theta provides an actor model implementation, designed for building resilient and high performance concurrent systems. The framework emphasizes the "let it crash" philosophy on top of as small as possible code for reasoning. 

## Design Goals
- **Ergonomics** - Time of developer (and token of AI) matters
- **Performance** - Performance along with simplicity

## Features
- **Async-first design** - Native integration with Tokio's async runtime
- **Built-in persistence** - Seamless respawn of actor from snapshot on file system, AWS S3 etc.
- **Built-in remote** - `ActorRef` could be passed around through `iroh` network, with seamless import and export on boundaries
- **WASM support** - Compile to WebAssembly for running in browser or other WASM environments

## Installation

```sh
cargo add theta
```

## 🚧 WIP
Theta is currently under active development and API is subject to change. Not recommended for production use yet.

## License

Licensed under the [MIT License](LICENSE).