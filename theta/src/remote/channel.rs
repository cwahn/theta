// Implement Serialze and Deserialize for ActorRef

// Make Connection Algorithm

use std::{any::Any, cell::RefCell, collections::BTreeMap};

use iroh::{Endpoint, PublicKey, endpoint::Connection};
use serde::Serialize;
use tokio::task_local;
use uuid::Uuid;

use crate::{actor::Actor, prelude::ActorRef};

// Task-local storage for collecting senders
task_local! {
    static SENDER_COLLECTOR: RefCell<Option<BTreeMap<Uuid, Box<dyn Any + Send>>>>;
}

pub struct RemoteManager {
    pub endpoint: Endpoint,
    pub connections: BTreeMap<PublicKey, Connection>,

    pub tx_sockets: BTreeMap<Uuid, TxSocket>,
    pub rx_sockets: BTreeMap<Uuid, RxSocket>,

    pub pending_rx_sockets: BTreeMap<Uuid, RxSocket>,
}

fn run() {
    tokio::spawn(async move {
        // Initialize the remote manager and start listening for connections
        let ep = Endpoint::builder()
            .alpns(vec![b"theta".into()])
            .bind()
            .await?;


    });
}

// ! Must be executed in tokio context
fn serialize_msg<M: Serialize>(
    msg: &M,
) -> Result<(Vec<u8>, BTreeMap<Uuid, Box<dyn Any + Send>>), postcard::Error> {
    let serialized = postcard::to_stdvec(msg)?;

    let collected = SENDER_COLLECTOR
        .with(|collector| collector.borrow_mut().take())
        .unwrap();

    Ok((serialized, collected))
}

// ! Must be executed in tokio context
impl<A> Serialize for ActorRef<A>
where
    A: Actor,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Collect the sender in the task-local storage
        let _ = SENDER_COLLECTOR.try_with(|collector| {
            if let Some(ref mut map) = *collector.borrow_mut() {
                map.insert(self.id, Box::new(self.tx.clone()));
            }
        });

        // Serialize only the UUID
        self.0.serialize(serializer)
    }
}
