//! Remote actor communication over peer-to-peer networks.
//!
//! This module enables distributed actor systems using the [iroh] P2P networking library.
//! Actors can communicate across network boundaries as if they were local.
//!
//! [iroh]: https://iroh.computer/

pub mod base;
pub mod network;
pub mod peer;
pub mod serde;
