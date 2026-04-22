use serde::{Deserialize, Serialize};
use theta::prelude::*;

const WORKER_UUID: uuid::Uuid = uuid::uuid!("a1b2c3d4-e5f6-7890-abcd-ef0123456789");
const MANAGER_UUID: uuid::Uuid = uuid::uuid!("b2c3d4e5-f6a7-8901-bcde-f01234567890");

#[derive(Debug, Clone, ActorArgs)]
pub struct Worker {
    pub id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping;

#[actor(WORKER_UUID)]
impl Actor for Worker {
    const _: () = async |_: Ping| -> u64 { self.id };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorkers;

#[derive(Debug, Clone, ActorArgs)]
pub struct Manager {
    pub workers: Vec<ActorRef<Worker>>,
}

#[actor(MANAGER_UUID)]
impl Actor for Manager {
    const _: () = async |_: GetWorkers| -> Vec<ActorRef<Worker>> { self.workers.clone() };
}
