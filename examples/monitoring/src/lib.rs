use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use theta::prelude::*;
use tracing::info;

const COUNTER_UUID: uuid::Uuid = uuid::uuid!("a1b2c3d4-5e6f-7890-abcd-ef1234567890");

#[derive(Debug, Clone, Default, Hash, ActorArgs, Serialize, Deserialize)]
pub struct Counter {
    pub value: i64,
}

impl Counter {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inc {
    pub amount: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dec {
    pub amount: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterResponse {
    pub new_value: i64,
}

#[actor(COUNTER_UUID)]
impl Actor for Counter {
    type View = Self;

    const _: () = async |msg: Inc| -> CounterResponse {
        let new_value = self.value + msg.amount;

        self.value = new_value;

        info!("counter incremented by {} to {}", msg.amount, new_value);

        CounterResponse { new_value }
    };

    const _: () = async |msg: Dec| -> CounterResponse {
        let new_value = self.value - msg.amount;

        self.value = new_value;

        info!("counter decremented by {} to {}", msg.amount, new_value);

        CounterResponse { new_value }
    };

    fn hash_code(&self) -> u64 {
        let mut hasher = ahash::AHasher::default();

        Hash::hash(self, &mut hasher);

        hasher.finish()
    }
}
