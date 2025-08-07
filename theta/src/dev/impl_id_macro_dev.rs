use serde::{Deserialize, Serialize};

use theta_macros::{ActorConfig, impl_id};

use crate::{
    actor::{Actor, Nil},
    context::Context,
    signal::{Message, Continuation, Message},
    prelude::GlobalContext,
};

#[derive(Debug, Clone, ActorConfig, Hash)]
pub struct Manager {}

#[impl_id("da0c631b-e3d6-4369-bff2-80939f4ef177")]
impl Actor for Manager {
    type StateReport = Nil; // Which means reporting is no-op
}

// Message

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorker {
    pub name: String,
}

#[impl_id("3b2b8513-bb43-4bc5-a6d7-ea392df44ce0")]
impl Message<CreateWorker> for Manager {
    type Return = ();

    async fn process(&mut self, _ctx: Context<Self>, msg: CreateWorker) -> Self::Return {
        println!("Creating worker with name: {}", msg.name);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorker {
    pub name: String,
}

#[impl_id("5c3b8513-bb43-4bc5-a6d7-ea392df44ce1")]
impl Message<GetWorker> for Manager {
    type Return = Option<String>; // Assuming worker returns a name

    async fn process(&mut self, _ctx: Context<Self>, msg: GetWorker) -> Self::Return {
        println!("Getting worker with name: {}", msg.name);
        Some(msg.name) // Placeholder return
    }
}

#[tokio::test]
async fn test_manager_behavior() {
    let ctx = GlobalContext::initialize().await;

    let msgs: Vec<Box<dyn Message<Manager>>> = vec![
        Box::new(CreateWorker {
            name: "Worker1".to_string(),
        }),
        Box::new(GetWorker {
            name: "Worker1".to_string(),
        }),
    ];

    let serialized_msgs = msgs
        .iter()
        .map(|msg| postcard::to_allocvec_cobs(msg).unwrap())
        .collect::<Vec<_>>();

    let deserialized_msgs: Vec<Box<dyn Message<Manager>>> = serialized_msgs
        .into_iter()
        .map(|mut msg| postcard::from_bytes_cobs(msg.as_mut_slice()).unwrap())
        .collect();

    let manager = ctx.spawn(Manager {}).await;

    for msg in deserialized_msgs {
        let _ = manager.send_raw(msg, Continuation::nil());
    }
}
