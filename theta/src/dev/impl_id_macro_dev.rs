use serde::{Deserialize, Serialize};

use theta_macros::{ActorConfig, impl_id};

use crate::{
    actor::Actor,
    context::Context,
    message::{Behavior, Continuation, Message},
    prelude::GlobalContext,
    remote::RegisterActorFn,
};

// Root should call actor initilization

// Each actor should register function to form it's own registry
// Actor::__IMPL_ID -> fn ()

// Should contribute to the deserialization registry of Message<Manager>
// Only thing to identify Actor is impl_id
// Actor::__IMPL_ID -> (Behavior::__IMPL_ID -> DeserializeFn<>)
// Contribute as (__IMPL_ID, __IMPL_ID, fn mut (Box<dyn Any + Send>))

// type ActorInitFn = fn();

#[derive(Debug, Clone, ActorConfig)]
pub struct Manager {}

#[impl_id("da0c631b-e3d6-4369-bff2-80939f4ef177")]
impl Actor for Manager {}

// Message

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorker {
    pub name: String,
}

#[impl_id("3b2b8513-bb43-4bc5-a6d7-ea392df44ce0")]
impl Behavior<CreateWorker> for Manager {
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
impl Behavior<GetWorker> for Manager {
    type Return = Option<String>; // Assuming worker returns a name

    async fn process(&mut self, _ctx: Context<Self>, msg: GetWorker) -> Self::Return {
        println!("Getting worker with name: {}", msg.name);
        Some(msg.name) // Placeholder return
    }
}

#[tokio::test]
async fn test_manager_behavior() {
    // Run initialization
    // for init_fn in inventory::iter::<RegisterActorFn> {
    //     (init_fn.0)();
    // }

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
        .map(|msg| serde_json::to_string(msg).unwrap())
        .collect::<Vec<_>>();

    let deserialized_msgs: Vec<Box<dyn Message<Manager>>> = serialized_msgs
        .iter()
        .map(|msg| serde_json::from_str(msg).unwrap())
        .collect();

    let manager = ctx.spawn(Manager {}).await;

    for msg in deserialized_msgs {
        let _ = manager.send_dyn(msg, Continuation::nil());
    }
}
