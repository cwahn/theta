use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use theta_macros::{ActorArgs, actor};

use crate::{
    actor::Actor, base::Nil, context::RootContext, message::Continuation, prelude::ActorRef,
};

#[derive(Debug, Clone, ActorArgs)]
pub struct Manager {
    workers: HashMap<String, ActorRef<Worker>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorker {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorker {
    pub name: String,
}

#[derive(Debug, Clone, ActorArgs)]
pub struct Worker {}

#[actor("27ca7f4a-f2f7-4644-8ff9-4bdd8f40b5cd")]
impl Actor for Worker {
    type View = Nil; // Which means reporting is no-op
}

#[actor("d89de30e-79c5-49b6-9c16-0903ac576277")]
impl Actor for Manager {
    type View = Nil; // Which means reporting is no-op

    const _: () = {
        async |msg: CreateWorker| -> () {
            println!("Creating worker with name: {}", msg.name);

            let worker = ctx.spawn(Worker {});

            self.workers.insert(msg.name.clone(), worker.clone());
        };

        async |GetWorker { name }| -> Option<ActorRef<Worker>> {
            println!("Getting worker with name: {name}");

            if let Some(worker) = self.workers.get(&name).cloned() {
                Some(worker)
            } else {
                println!("Worker with name {name} not found");
                None
            }
        };
    };
}

#[tokio::test]
async fn test_manager_behavior() {
    let ctx = RootContext::default();

    let msgs: Vec<<Manager as Actor>::Msg> = vec![
        CreateWorker {
            name: "Worker1".to_string(),
        }
        .into(),
        GetWorker {
            name: "Worker1".to_string(),
        }
        .into(),
    ];

    let serialized_msgs = msgs
        .iter()
        .map(|m| postcard::to_allocvec_cobs(m).unwrap())
        .collect::<Vec<_>>();

    let deserialized_msgs: Vec<<Manager as Actor>::Msg> = serialized_msgs
        .into_iter()
        .map(|mut m| postcard::from_bytes_cobs(m.as_mut_slice()).unwrap())
        .collect();

    let manager = ctx.spawn(Manager {
        workers: HashMap::new(),
    });

    for msg in deserialized_msgs {
        let _ = manager.send_raw(msg, Continuation::Nil);
    }
}
