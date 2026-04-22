use iroh::PublicKey;
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use tracing::info;

const PING_PONG_UUID: uuid::Uuid = uuid::uuid!("f68fe56f-8aa9-4f90-8af8-591a06e2818a");

#[derive(Debug, Clone, ActorArgs)]
pub struct PingPong;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping {
    pub source: PublicKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pong {}

#[actor(PING_PONG_UUID)]
impl Actor for PingPong {
    const _: () = async |msg: Ping| -> Pong {
        info!("received ping from {}", msg.source);

        Pong {}
    };
}
