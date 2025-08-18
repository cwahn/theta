//! Remote actor communication over peer-to-peer networks.
//!
//! This module enables distributed actor systems using the [iroh] P2P networking library.
//! Actors can communicate across network boundaries as if they were local.
//!
//! # Features
//!
//! - **P2P networking**: Direct peer-to-peer communication without central servers
//! - **Transparent remoting**: `ActorRef` works the same for local and remote actors
//! - **Actor discovery**: Look up actors by name on remote peers
//! - **Message serialization**: Automatic handling of message serialization/deserialization
//!
//! [iroh]: https://iroh.computer/

pub mod base;
pub mod network;
pub mod peer;
pub mod serde;
