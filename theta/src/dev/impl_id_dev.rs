use std::{
    any::Any,
    sync::{Arc, LazyLock, RwLock},
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_flexitos::Registry;
use theta_macros::ActorConfig;
use uuid::uuid;

use crate::{
    actor::Actor,
    base::ImplId,
    message::{Behavior, Continuation, DynMessage, Message},
    prelude::GlobalContext,
    remote::serde::DeserializeFnRegistry,
};

// Root should call actor initilization

// Each actor should register function to form it's own registry
// Actor::__IMPL_ID -> fn ()

// Should contribute to the deserialization registry of Message<Manager>
// Only thing to identify Actor is impl_id
// Actor::__IMPL_ID -> (Behavior::__IMPL_ID -> DeserializeFn<>)
// Contribute as (__IMPL_ID, __IMPL_ID, fn mut (Box<dyn Any + Send>))

// type ActorInitFn = fn();
struct ActorInitFn(fn());
inventory::collect!(ActorInitFn);

struct MessageDeserializeEntry {
    actor_impl_id: ImplId,
    register_fn: fn(&mut Box<dyn Any + Send + Sync>), // Which will be each actor's deserialize function registry
}
inventory::collect!(MessageDeserializeEntry);

static TYPE_SYSTEM: LazyLock<RwLock<FxHashMap<ImplId, Box<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

#[derive(Debug, Clone, ActorConfig)]
pub struct Manager {}

impl Actor for Manager {
    const __IMPL_ID: ImplId = uuid!("da0c631b-e3d6-4369-bff2-80939f4ef177");
}

inventory::submit! {
    ActorInitFn(||{
        let mut registry = Box::new(DeserializeFnRegistry::<dyn Message<Manager>>::new())
            as Box<dyn Any + Send + Sync>;

        for entry in inventory::iter::<MessageDeserializeEntry> {
            if entry.actor_impl_id == <Manager as Actor>::__IMPL_ID {
                (entry.register_fn)(&mut registry);
            }
        }

        // Commit to the global type system
        TYPE_SYSTEM.write().unwrap().insert(
            <Manager as Actor>::__IMPL_ID,
            registry,
        );
    })
}

impl Serialize for dyn Message<Manager> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        const fn __check_erased_serialize_supertrait<T: ?Sized + Message<Manager>>() {
            ::serde_flexitos::ser::require_erased_serialize_impl::<T>();
        }

        ::serde_flexitos::serialize_trait_object(serializer, self.__impl_id(), self)
    }
}

impl<'de> Deserialize<'de> for Box<dyn Message<Manager>> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let type_system = TYPE_SYSTEM.read().unwrap();

        let registry = type_system
            .get(&<Manager as Actor>::__IMPL_ID)
            .and_then(|v| v.downcast_ref::<DeserializeFnRegistry<dyn Message<Manager>>>())
            .ok_or_else(|| {
                serde::de::Error::custom("Failed to get DeserializeFnRegistry for Manager")
            })?;

        registry.deserialize_trait_object(deserializer)
    }
}

// Message

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorker {
    pub name: String,
}

impl Behavior<CreateWorker> for Manager {
    type Return = ();

    async fn process(
        &mut self,
        _ctx: crate::context::Context<Self>,
        msg: CreateWorker,
    ) -> Self::Return {
        // Logic to create a worker
        println!("Creating worker with name: {}", msg.name);
    }

    const __IMPL_ID: ImplId = uuid!("4b3b8513-bb43-4bc5-a6d7-ea392df44ce0");
}

inventory::submit! {
    MessageDeserializeEntry {
        actor_impl_id: <Manager as Actor>::__IMPL_ID,
        register_fn: |registry| {
            if let Some(reg) = registry.downcast_mut::<DeserializeFnRegistry<dyn Message<Manager>>>() {
                reg.register(
                    <Manager as Behavior<CreateWorker>>::__IMPL_ID,
                    |d| {
                        Ok(Box::new(
                            erased_serde::deserialize::<CreateWorker>(d)?,
                        ))
                    },
                );
            } else {
                panic!("Failed to downcast registry to DeserializeFnRegistry<dyn Message<Manager>>");
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWorker {
    pub name: String,
}

impl Behavior<GetWorker> for Manager {
    type Return = Option<String>; // Assuming worker returns a name

    async fn process(
        &mut self,
        _ctx: crate::context::Context<Self>,
        msg: GetWorker,
    ) -> Self::Return {
        // Logic to get a worker
        println!("Getting worker with name: {}", msg.name);
        Some(msg.name) // Placeholder return
    }

    const __IMPL_ID: ImplId = uuid!("5c3b8513-bb43-4bc5-a6d7-ea392df44ce1");
}

inventory::submit! {
    MessageDeserializeEntry {
        actor_impl_id: <Manager as Actor>::__IMPL_ID,
        register_fn: |registry| {
            if let Some(reg) = registry.downcast_mut::<DeserializeFnRegistry<dyn Message<Manager>>>() {
                reg.register(
                    <Manager as Behavior<GetWorker>>::__IMPL_ID,
                    |d| {
                        Ok(Box::new(
                            erased_serde::deserialize::<GetWorker>(d)?,
                        ))
                    },
                );
            } else {
                panic!("Failed to downcast registry to DeserializeFnRegistry<dyn Message<Manager>>");
            }
        }
    }
}

#[tokio::test]
async fn test_manager_behavior() {
    // Run initialization
    for init_fn in inventory::iter::<ActorInitFn> {
        (init_fn.0)();
    }

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
