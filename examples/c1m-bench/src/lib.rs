use serde::{Deserialize, Serialize};
use theta::prelude::*;

#[derive(Debug, Clone, ActorArgs)]
pub struct Worker {
    pub id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping;

#[actor("a1b2c3d4-e5f6-7890-abcd-ef0123456789")]
impl Actor for Worker {
    const _: () = async |_: Ping| -> u64 { self.id };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorkers;

#[derive(Debug, Clone, ActorArgs)]
pub struct Manager {
    pub workers: Vec<ActorRef<Worker>>,
}

#[actor("b2c3d4e5-f6a7-8901-bcde-f01234567890")]
impl Actor for Manager {
    const _: () = async |_: GetWorkers| -> Vec<ActorRef<Worker>> { self.workers.clone() };
}
