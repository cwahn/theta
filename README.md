# Theta

[![Crates.io](https://img.shields.io/crates/v/theta.svg)](https://crates.io/crates/theta)
[![Documentation](https://docs.rs/theta/badge.svg)](https://docs.rs/theta)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **Async actor framework for Rust with supervision and fault tolerance**

Theta provides a lightweight, async-first actor model implementation inspired by Erlang/OTP, built on top of Tokio. Design philosophy: *let it crash*, but with style.

## ✨ Features

- **Async-native** - Built from the ground up for `async`/`.await`
- **Supervision trees** - Hierarchical fault tolerance with configurable restart strategies  
- **Panic safety** - Actors crash gracefully without taking down the system
- **Escalation handling** - Failures bubble up through supervision hierarchy
- **Lifecycle management** - Clean initialization, restart, and termination hooks
- **Zero-copy messaging** - Efficient message passing between actors

## 🚀 Quick Start

Add to your `Cargo.toml`:

```sh
cargo add theta
```

## 🏗️ Architecture

Theta follows the actor model where:

- **Actors** are isolated units of computation with private state
- **Messages** are the only way actors communicate
- **Supervision** provides fault tolerance through restart strategies


## 📄 License

Licensed under [MIT License](LICENSE).
