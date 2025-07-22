use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, LazyLock, RwLock},
    time::Duration,
};
use tokio::{sync::mpsc::UnboundedSender, task_local};
use uuid::Uuid;

// Your fixed types
pub type Continuation = tokio::sync::oneshot::Sender<Box<dyn Any + Send>>;
pub type DynMessage<A> = Box<dyn Message<A>>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Actor: Send + 'static + std::fmt::Debug {
    const ACTOR_TYPE_ID: &'static str;
}

pub trait Message<A>: std::fmt::Debug + Send
where
    A: Actor,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send>>;
}

#[derive(Debug)]
pub struct ActorRef<A: Actor> {
    pub id: Uuid,
    pub tx: UnboundedSender<(DynMessage<A>,Continuation)>,
}

pub struct Context<A> {
    _phantom: std::marker::PhantomData<A>,
}

// Use Arc instead of Box for cloneable type-erased handles
type DynTx = Arc<dyn Any + Send + Sync>;
type DynRx = Arc<dyn Any + Send + Sync>;

// Connection state f or different actor types
static INBOUND_CONNECTIONS: LazyLock<RwLock<FxHashMap<&'static str, FxHashMap<Uuid, DynRx>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

static OUTBOUND_CONNECTIONS: LazyLock<RwLock<FxHashMap<&'static str, FxHashMap<Uuid, DynTx>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

// Deserialization function registry per actor type
type DeserializeFn<A> =
    fn(&[u8]) -> Result<DynMessage<A>, Box<dyn std::error::Error + Send + Sync>>;

static DESERIALIZE_REGISTRY: LazyLock<RwLock<FxHashMap<&'static str, Arc<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

// Task-local buffers for connection establishment during serde
task_local! {
    // Connections established during serialization (outbound)
    static SERIALIZATION_OUTBOUND_BUF: RefCell<Option<FxHashMap<(&'static str, Uuid), DynTx>>>;
    // Connections established during deserialization (inbound)
    static DESERIALIZATION_INBOUND_BUF: RefCell<Option<FxHashMap<(&'static str, Uuid), DynRx>>>;
}

// Register a deserialize function for an actor type
pub fn register_actor_deserializer<A: Actor>(deserialize_fn: DeserializeFn<A>) {
    let mut registry = DESERIALIZE_REGISTRY.write().unwrap();
    registry.insert(
        A::ACTOR_TYPE_ID,
        Arc::new(deserialize_fn) as Arc<dyn Any + Send + Sync>,
    );
}

// Establish inbound connection for an actor (called during serialization of outbound ActorRef)
fn establish_inbound_connection<A: Actor>(actor_id: Uuid, actor_ref: &ActorRef<A>) -> DynRx {
    // Create the connection handle for this specific actor type
    let connection_handle = Arc::new(actor_ref.tx.clone()) as DynRx;

    // Store in task-local buffer for batch processing
    let _ = SERIALIZATION_OUTBOUND_BUF.try_with(|buf| {
        if let Some(ref mut map) = *buf.borrow_mut() {
            map.insert((A::ACTOR_TYPE_ID, actor_id), connection_handle.clone());
        } else {
            // Initialize if not set
            buf.borrow_mut().replace({
                let mut new_map = FxHashMap::default();
                new_map.insert((A::ACTOR_TYPE_ID, actor_id), connection_handle.clone());
                new_map
            });
        }
    });

    connection_handle
}

// Establish outbound connection for an actor (called during deserialization of inbound ActorRef)
fn establish_outbound_connection<A: Actor>(actor_id: Uuid) -> Result<DynTx, ConnectionError> {
    // Check if we already have an outbound connection
    {
        let outbound = OUTBOUND_CONNECTIONS.read().unwrap();
        if let Some(type_map) = outbound.get(A::ACTOR_TYPE_ID) {
            if let Some(connection) = type_map.get(&actor_id) {
                return Ok(connection.clone());
            }
        }
    }

    // Create new outbound connection (this would integrate with your network layer)
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<(DynMessage<A>,Continuation)>();
    let connection_handle = Arc::new(tx) as DynTx;

    // Store in task-local buffer for batch processing
    let _ = DESERIALIZATION_INBOUND_BUF.try_with(|buf| {
        if let Some(ref mut map) = *buf.borrow_mut() {
            map.insert((A::ACTOR_TYPE_ID, actor_id), connection_handle.clone());
        } else {
            // Initialize if not set
            buf.borrow_mut().replace({
                let mut new_map = FxHashMap::default();
                new_map.insert((A::ACTOR_TYPE_ID, actor_id), connection_handle.clone());
                new_map
            });
        }
    });

    // TODO: Spawn task to handle the rx side - forward to actual network connection
    tokio::spawn(async move {
        // This would forward messages to the actual network stream
        // handle_outbound_messages(rx, actor_id).await;
        drop(rx); // Prevent unused variable warning for now
    });

    Ok(connection_handle)
}

// Serialization for ActorRef - establishes inbound connections
impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Establish inbound connection for this actor reference
        establish_inbound_connection(self.id, self);

        // Serialize the actor reference as (type_id, uuid)
        let actor_data = (A::ACTOR_TYPE_ID, self.id);
        actor_data.serialize(serializer)
    }
}

// Deserialization for ActorRef - establishes outbound connections
impl<'de, A: Actor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (actor_type_id, actor_id): (&str, Uuid) = Deserialize::deserialize(deserializer)?;

        // Verify the actor type matches
        if actor_type_id != A::ACTOR_TYPE_ID {
            return Err(serde::de::Error::custom(format!(
                "Actor type mismatch: expected {}, got {}",
                A::ACTOR_TYPE_ID,
                actor_type_id
            )));
        }

        // Establish outbound connection
        let connection_handle = establish_outbound_connection::<A>(actor_id).map_err(|e| {
            serde::de::Error::custom(format!("Failed to establish connection: {:?}", e))
        })?;

        // Extract the typed sender from the connection handle
        let tx = connection_handle
            .downcast_ref::<UnboundedSender<(DynMessage<A>,Continuation)>>()
            .ok_or_else(|| serde::de::Error::custom("Connection handle type mismatch"))?
            .clone();

        Ok(ActorRef { id: actor_id, tx })
    }
}

// High-level serialization function that manages connections
pub async fn serialize_with_connections<T: Serialize>(
    value: &T,
) -> Result<(Vec<u8>, FxHashMap<(&'static str, Uuid), DynRx>), SerializationError> {
    // Initialize serialization buffer in a task context
    let result = SERIALIZATION_OUTBOUND_BUF
        .scope(RefCell::new(Some(FxHashMap::default())), async move {
            // Serialize the value
            let serialized = postcard::to_stdvec(value)?;

            // Extract established connections
            let connections =
                SERIALIZATION_OUTBOUND_BUF.with(|buf| buf.borrow().clone().unwrap_or_default());

            Ok((serialized, connections))
        })
        .await;

    result
}

// High-level deserialization function that manages connections
pub async fn deserialize_with_connections<T>(
    data: &[u8],
) -> Result<(T, FxHashMap<(&'static str, Uuid), DynTx>), DeserializationError>
where
    T: for<'de> Deserialize<'de>,
{
    // Initialize deserialization buffer in a task context
    let result = DESERIALIZATION_INBOUND_BUF
        .scope(RefCell::new(Some(FxHashMap::default())), async move {
            // Deserialize the value
            let deserialized: T = postcard::from_bytes(data)?;

            // Extract established connections
            let connections =
                DESERIALIZATION_INBOUND_BUF.with(|buf| buf.borrow().clone().unwrap_or_default());

            Ok((deserialized, connections))
        })
        .await;

    result
}

// Function to deserialize messages using registered deserializers
pub fn deserialize_message_for_actor(
    actor_type_id: &str,
    _data: &[u8],
) -> Result<Box<dyn Any + Send>, MessageDeserializationError> {
    let registry = DESERIALIZE_REGISTRY.read().unwrap();
    let _any_deserializer = registry
        .get(actor_type_id)
        .ok_or(MessageDeserializationError::UnknownActorType)?;

    // This is where the magic happens - we use Any to call the typed deserializer
    // The actual implementation would need to be more sophisticated
    // For now, this is a placeholder that shows the concept
    todo!("Implement typed message deserialization using Any downcast")
}

// Commit connections from buffers to global state
pub fn commit_inbound_connections(connections: FxHashMap<(&'static str, Uuid), DynRx>) {
    let mut inbound = INBOUND_CONNECTIONS.write().unwrap();
    for ((actor_type, actor_id), connection) in connections {
        let type_map = inbound.entry(actor_type).or_insert_with(FxHashMap::default);
        type_map.insert(actor_id, connection);
    }
}

pub fn commit_outbound_connections(connections: FxHashMap<(&'static str, Uuid), DynTx>) {
    let mut outbound = OUTBOUND_CONNECTIONS.write().unwrap();
    for ((actor_type, actor_id), connection) in connections {
        let type_map = outbound
            .entry(actor_type)
            .or_insert_with(FxHashMap::default);
        type_map.insert(actor_id, connection);
    }
}

// Error types
#[derive(Debug)]
pub enum ConnectionError {
    NetworkError,
    Timeout,
}

#[derive(Debug)]
pub enum SerializationError {
    PostcardError(postcard::Error),
}

impl From<postcard::Error> for SerializationError {
    fn from(e: postcard::Error) -> Self {
        SerializationError::PostcardError(e)
    }
}

#[derive(Debug)]
pub enum DeserializationError {
    PostcardError(postcard::Error),
}

impl From<postcard::Error> for DeserializationError {
    fn from(e: postcard::Error) -> Self {
        DeserializationError::PostcardError(e)
    }
}

#[derive(Debug)]
pub enum MessageDeserializationError {
    UnknownActorType,
    DeserializationFailed,
}

// Example usage
#[derive(Debug)]
pub struct MyActor {
    name: String,
}

impl Actor for MyActor {
    const ACTOR_TYPE_ID: &'static str = "MyActor";
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MyMessage {
    content: String,
}

impl Message<MyActor> for MyMessage {
    fn process_dyn<'a>(
        self: Box<Self>,
        _ctx: Context<MyActor>,
        _state: &'a mut MyActor,
    ) -> BoxFuture<'a, Box<dyn Any + Send>> {
        Box::pin(async move {
            println!("Processing: {:?}", self);
            Box::new(()) as Box<dyn Any + Send>
        })
    }
}

// Example deserializer function for MyActor
fn deserialize_my_actor_message(
    data: &[u8],
) -> Result<DynMessage<MyActor>, Box<dyn std::error::Error + Send + Sync>> {
    let message: MyMessage = postcard::from_bytes(data)?;
    Ok(Box::new(message))
}

#[tokio::main]
async fn main() {
    // Register deserializer
    register_actor_deserializer::<MyActor>(deserialize_my_actor_message);

    // Create actor ref
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let actor_ref = ActorRef::<MyActor> {
        id: Uuid::new_v4(),
        tx,
    };

    // Serialize with connection establishment
    let (serialized, inbound_connections) = serialize_with_connections(&actor_ref).await.unwrap();
    println!(
        "Serialized actor ref, established {} inbound connections",
        inbound_connections.len()
    );

    // Commit connections
    commit_inbound_connections(inbound_connections);

    // Deserialize with connection establishment
    let (deserialized_ref, outbound_connections): (ActorRef<MyActor>, _) =
        deserialize_with_connections(&serialized).await.unwrap();
    println!(
        "Deserialized actor ref, established {} outbound connections",
        outbound_connections.len()
    );

    // Commit connections
    commit_outbound_connections(outbound_connections);

    println!("Original ID: {}", actor_ref.id);
    println!("Deserialized ID: {}", deserialized_ref.id);
    println!("IDs match: {}", actor_ref.id == deserialized_ref.id);

    println!("Test completed successfully!");
}
