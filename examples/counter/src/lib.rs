use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use theta::prelude::*;

const COUNTER_UUID: uuid::Uuid = uuid::uuid!("96d9901f-24fc-4d82-8eb8-023153d41074");
const MANAGER_UUID: uuid::Uuid = uuid::uuid!("f65b84e6-adfe-4d3a-8140-ee55de512070");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dec;

#[derive(Debug, Clone, ActorArgs)]
pub struct Counter {
    pub value: i64,
}

#[actor(COUNTER_UUID)]
impl Actor for Counter {
    type View = i64;

    const _: () = async |_: Inc| -> i64 {
        self.value += 1;

        self.value
    };

    const _: () = async |_: Dec| -> i64 {
        self.value -= 1;

        self.value
    };

    fn hash_code(&self) -> u64 {
        let mut hasher = ahash::AHasher::default();

        self.value.hash(&mut hasher);

        hasher.finish()
    }
}

impl From<&Counter> for i64 {
    fn from(counter: &Counter) -> Self {
        counter.value
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorker;

#[derive(Debug, Clone, Hash, ActorArgs, Serialize, Deserialize)]
pub struct Manager {
    pub worker: ActorRef<Counter>,
}

#[actor(MANAGER_UUID)]
impl Actor for Manager {
    type View = Self;

    const _: () = async |_: GetWorker| -> ActorRef<Counter> { self.worker.clone() };
}
