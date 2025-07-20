# Theta

[![Crates.io](https://img.shields.io/crates/v/theta.svg)](https://crates.io/crates/theta)
[![Documentation](https://docs.rs/theta/badge.svg)](https://docs.rs/theta)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**A robust async actor framework for Rust with supervision and fault tolerance**

Theta provides an actor model implementation, designed for building resilient and high performance concurrent systems. The framework emphasizes the "let it crash" philosophy on top of as small as possible code for reasoning. 

## Features

- **Async-first design** - Native integration with Tokio's async runtime
- **Supervision trees** - Panics are contained and handled gracefully through supervision hierarchy
- **Efficient messaging** - High-performance message passing between actors

## Installation

```sh
cargo add theta
```

## Architecture

Theta implements the actor model with the following core principles:

- **Actors** are essentially a sigle tokio task with two mpsc channels
- **ActorRef** is an address of an actor to send messages
- **Supervision tree** provides a hierarchy of actors for managing failures

## Core Components

**Actor Trait**
- `initialize()` - Actor initialization with provided arguments
- `process_msg()` - Message processing with panic safety
- `supervise()` - Supervision strategy for child actor failures
- `on_restart()` - Pre-restart cleanup and preparation
- `on_exit()` - Final cleanup on actor termination

**Supervision Model**
- Panic of an actor escalates to its parent supervisor
- Parent decides how to handle the failure for the failed actor and the rest of the children

## License

Licensed under the [MIT License](LICENSE).