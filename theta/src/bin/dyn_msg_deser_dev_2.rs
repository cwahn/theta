use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_flexitos::{MapRegistry, Registry, serialize_trait_object};
use std::collections::HashMap;
use std::sync::LazyLock;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use uuid::Uuid;

// Your existing types
pub type Continuation = oneshot::Sender<Box<dyn std::any::Any + Send>>;
pub type DynMessage<A> = Box<dyn Message<A>>;

pub trait Actor: Send + 'static {
    // Each actor type needs a unique string identifier
    const ACTOR_TYPE_ID: &'static str;
}

pub trait Message<A: Actor>: erased_serde::Serialize + std::fmt::Debug + Send {
    // Each message type needs a unique key for the given actor type
    fn message_key(&self) -> &'static str;
}

#[derive(Debug)]
pub struct ActorRef<A: Actor> {
    pub(crate) id: Uuid,
    pub(crate) tx: UnboundedSender<(DynMessage<A>,Continuation)>,
}

// Key insight: Store channels per actor TYPE, not per actor instance
static ACTOR_CHANNELS: LazyLock<
    std::sync::RwLock<HashMap<&'static str, HashMap<Uuid, Box<dyn std::any::Any + Send + Sync>>>>,
> = LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

// Registry per actor type - this is the key pattern from serde_flexitos!
macro_rules! create_message_registry {
    ($actor_type:ty) => {
        paste::paste! {
            static [<$actor_type:upper _MESSAGE_REGISTRY>]: LazyLock<MapRegistry<dyn Message<$actor_type>>> = LazyLock::new(|| {
                let mut registry = MapRegistry::<dyn Message<$actor_type>>::new(stringify!($actor_type));
                // Register message types here - could be done by a macro
                registry
            });
        }
    };
}

// Implement Serialize for dynamic messages (like the example)
impl<A: Actor> Serialize for dyn Message<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_trait_object(serializer, self.message_key(), self)
    }
}

// The magic function: deserialize and send directly to the actor's channel
pub async fn handle_network_message_for_actor<A: Actor>(
    actor_id: Uuid,
    binary_data: &[u8],
    registry: &MapRegistry<dyn Message<A>>,
) -> Result<(), NetworkMessageError>
where
    for<'de> DynMessage<A>: Deserialize<'de>,
{
    // 1. Deserialize using the actor-specific registry
    let message: DynMessage<A> = postcard::from_bytes(binary_data)
        .map_err(|_| NetworkMessageError::DeserializationFailed)?;

    // 2. Get the actor's channel from storage
    let channels = ACTOR_CHANNELS.read().unwrap();
    let actor_type_channels = channels
        .get(A::ACTOR_TYPE_ID)
        .ok_or(NetworkMessageError::ActorTypeNotFound)?;

    let any_tx = actor_type_channels
        .get(&actor_id)
        .ok_or(NetworkMessageError::ActorNotFound)?;

    // 3. Downcast to the correct channel type (we know A at compile time here!)
    let tx = any_tx
        .downcast_ref::<UnboundedSender<(DynMessage<A>, Option<Continuation>)>>()
        .ok_or(NetworkMessageError::ChannelTypeMismatch)?;

    // 4. Send directly to the actor's channel
    tx.send((message, None))
        .map_err(|_| NetworkMessageError::ChannelClosed)?;

    Ok(())
}

// Router function that dispatches to the correct actor type handler
pub async fn route_network_message(
    actor_type_id: &str,
    actor_id: Uuid,
    binary_data: &[u8],
) -> Result<(), NetworkMessageError> {
    match actor_type_id {
        MyActor::ACTOR_TYPE_ID => {
            handle_network_message_for_actor::<MyActor>(
                actor_id,
                binary_data,
                &*MY_ACTOR_MESSAGE_REGISTRY,
            )
            .await
        }
        OtherActor::ACTOR_TYPE_ID => {
            handle_network_message_for_actor::<OtherActor>(
                actor_id,
                binary_data,
                &*OTHER_ACTOR_MESSAGE_REGISTRY,
            )
            .await
        }
        // Add more actor types as needed
        _ => Err(NetworkMessageError::UnknownActorType),
    }
}

// Register an actor's channel
pub fn register_actor<A: Actor>(actor_ref: &ActorRef<A>) {
    let mut channels = ACTOR_CHANNELS.write().unwrap();
    let actor_type_channels = channels
        .entry(A::ACTOR_TYPE_ID)
        .or_insert_with(HashMap::new);
    actor_type_channels.insert(
        actor_ref.id,
        Box::new(actor_ref.tx.clone()) as Box<dyn std::any::Any + Send + Sync>,
    );
}

// Enhanced registration with automatic message registry setup
pub fn register_actor_with_messages<A: Actor>(
    actor_ref: &ActorRef<A>,
    _registry: &mut MapRegistry<dyn Message<A>>,
) {
    register_actor(actor_ref);
    // Registry setup could be done here or via macro
}

#[derive(Debug)]
pub enum NetworkMessageError {
    UnknownActorType,
    ActorTypeNotFound,
    ActorNotFound,
    DeserializationFailed,
    ChannelTypeMismatch,
    ChannelClosed,
}

// Example actor and message types
pub struct MyActor;
impl Actor for MyActor {
    const ACTOR_TYPE_ID: &'static str = "MyActor";
}

pub struct OtherActor;
impl Actor for OtherActor {
    const ACTOR_TYPE_ID: &'static str = "OtherActor";
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HelloMessage {
    content: String,
}

impl Message<MyActor> for HelloMessage {
    fn message_key(&self) -> &'static str {
        "HelloMessage"
    }
}

// Add the required Into implementation
impl Into<Box<dyn Message<MyActor>>> for HelloMessage {
    fn into(self) -> Box<dyn Message<MyActor>> {
        Box::new(self)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PingMessage;

impl Message<MyActor> for PingMessage {
    fn message_key(&self) -> &'static str {
        "PingMessage"
    }
}

impl Message<OtherActor> for PingMessage {
    fn message_key(&self) -> &'static str {
        "PingMessage"
    }
}

// Add the required Into implementations for both actor types
impl Into<Box<dyn Message<MyActor>>> for PingMessage {
    fn into(self) -> Box<dyn Message<MyActor>> {
        Box::new(self)
    }
}

impl Into<Box<dyn Message<OtherActor>>> for PingMessage {
    fn into(self) -> Box<dyn Message<OtherActor>> {
        Box::new(self)
    }
}

// Create registries for each actor type
create_message_registry!(MyActor);
create_message_registry!(OtherActor);

// Manual registry setup (could be automated with macros)
static MY_ACTOR_MESSAGE_REGISTRY: LazyLock<MapRegistry<dyn Message<MyActor>>> =
    LazyLock::new(|| {
        let mut registry = MapRegistry::<dyn Message<MyActor>>::new("MyActor");
        registry.register_type::<HelloMessage>("HelloMessage");
        registry.register_type::<PingMessage>("PingMessage");
        registry
    });

static OTHER_ACTOR_MESSAGE_REGISTRY: LazyLock<MapRegistry<dyn Message<OtherActor>>> =
    LazyLock::new(|| {
        let mut registry = MapRegistry::<dyn Message<OtherActor>>::new("OtherActor");
        registry.register_type::<PingMessage>("PingMessage");
        registry
    });

// Implement Deserialize for each actor's message type
impl<'de> Deserialize<'de> for DynMessage<MyActor> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        MY_ACTOR_MESSAGE_REGISTRY.deserialize_trait_object(deserializer)
    }
}

impl<'de> Deserialize<'de> for DynMessage<OtherActor> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        OTHER_ACTOR_MESSAGE_REGISTRY.deserialize_trait_object(deserializer)
    }
}

// Usage example
pub async fn example_network_handler() -> Result<(), NetworkMessageError> {
    // Simulate network message arrival
    let actor_type = "MyActor";
    let actor_id = Uuid::new_v4();

    // Create and serialize a message
    let hello_msg = HelloMessage {
        content: "Hello from network!".to_string(),
    };
    let serialized = postcard::to_stdvec(&hello_msg).unwrap();

    // Route the message to the correct actor
    route_network_message(actor_type, actor_id, &serialized).await?;

    Ok(())
}

// Macro to automate the boilerplate (advanced usage)
macro_rules! actor_message_system {
    (
        actors: {
            $(($actor:ty, $actor_id:literal) => {
                messages: [$($message:ty => $message_key:literal),* $(,)?]
            }),* $(,)?
        }
    ) => {
        // This would generate all the registries, Deserialize impls, and routing logic
        // Left as an exercise for implementation
    };
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[tokio::test]
//     async fn test_message_routing() {
//         // Create actor
//         let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
//         let actor_ref = ActorRef::<MyActor> {
//             id: Uuid::new_v4(),
//             tx,
//         };

//         // Register actor
//         register_actor(&actor_ref);

//         // Test routing (would need actual message handling)
//         let result =
//             route_network_message(MyActor::ACTOR_TYPE_ID, actor_ref.id, b"dummy_data").await;

//         // Should fail due to invalid data, but actor type should be found
//         assert!(matches!(
//             result,
//             Err(NetworkMessageError::DeserializationFailed)
//         ));
//     }
// }

#[tokio::main]
async fn main() {
    // Your async main logic he
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let actor_ref = ActorRef::<MyActor> {
        id: Uuid::new_v4(),
        tx,
    };

    // Register the actor
    register_actor(&actor_ref);

    let result = route_network_message(MyActor::ACTOR_TYPE_ID, actor_ref.id, b"dummy_data").await;

    match result {
        Ok(_) => println!("Message routed successfully!"),
        Err(e) => eprintln!("Failed to route message: {:?}", e),
    }
}
