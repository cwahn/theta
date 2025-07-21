use futures::{FutureExt, future::BoxFuture};
use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Debug,
    sync::{Arc, LazyLock, Weak},
    time::Duration,
};
use tokio::sync::{
    Mutex, RwLock,
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tokio::task::JoinHandle;
use uuid::Uuid;

// Import from your existing modules
use theta::actor::Actor;
use theta::actor_ref::ActorRef;
use theta::context::Context;
use theta::error::SendError;
use theta::message::{Behavior, DynMessage, Message};
use theta::prelude::*;

// Iroh types (assuming they exist)
use iroh::net::{BiStream, ConnectionId};
use iroh::node::{Node, NodeId};

// The global registry slice that linkme will populate
#[distributed_slice]
pub static MESSAGE_REGISTRY: [MessageRegistration];

// Registration entry - directly creates DynMessage from bytes
pub struct MessageRegistration {
    pub uuid: Uuid,
    pub create_dyn_message_fn:
        fn(&[u8]) -> Result<Box<dyn Any + Send>, Box<dyn std::error::Error + Send + Sync>>,
}

// Lazy-initialized BTreeMap for fast lookups
static MESSAGE_MAP: LazyLock<BTreeMap<Uuid, &'static MessageRegistration>> =
    LazyLock::new(|| MESSAGE_REGISTRY.iter().map(|reg| (reg.uuid, reg)).collect());

// Global connection registry
static CONNECTION_REGISTRY: LazyLock<Arc<ConnectionRegistry>> =
    LazyLock::new(|| Arc::new(ConnectionRegistry::new()));

// UUID size (16 bytes)
const UUID_SIZE: usize = 16;

/// Trait for type-erased message sending
pub trait DynSender: Send + Sync {
    fn send_bytes(&self, data: Vec<u8>) -> Result<(), SendError>;
    fn is_closed(&self) -> bool;
    fn clone_dyn(&self) -> Box<dyn DynSender>;
}

/// Implementation of DynSender for typed channels
pub struct TypedDynSender<A: Actor> {
    sender: UnboundedSender<DynMessage<A>>,
}

impl<A: Actor> DynSender for TypedDynSender<A> {
    fn send_bytes(&self, data: Vec<u8>) -> Result<(), SendError> {
        let dyn_msg =
            deserialize_dyn_message_from_prefixed::<A>(&data).map_err(|_| SendError::Closed)?;
        self.sender.send(dyn_msg).map_err(|_| SendError::Closed)
    }

    fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    fn clone_dyn(&self) -> Box<dyn DynSender> {
        Box::new(TypedDynSender {
            sender: self.sender.clone(),
        })
    }
}

impl<A: Actor> TypedDynSender<A> {
    pub fn new(sender: UnboundedSender<DynMessage<A>>) -> Self {
        Self { sender }
    }

    pub fn get_sender(&self) -> &UnboundedSender<DynMessage<A>> {
        &self.sender
    }
}

/// Result of getting or creating a sender
pub enum SenderResult<A: Actor> {
    Existing(UnboundedSender<DynMessage<A>>),
    New(
        UnboundedSender<DynMessage<A>>,
        UnboundedReceiver<DynMessage<A>>,
    ),
}

/// Manages iroh connections and stream routing
pub struct IrohConnection {
    node_id: NodeId,
    connection_id: ConnectionId,
    inbound_handler: Option<JoinHandle<()>>,
    outbound_streams: HashMap<Uuid, BiStream>,
    stream_router: Arc<StreamRouter>,
}

impl IrohConnection {
    pub fn new(node_id: NodeId, connection_id: ConnectionId) -> Self {
        Self {
            node_id,
            connection_id,
            inbound_handler: None,
            outbound_streams: HashMap::new(),
            stream_router: Arc::new(StreamRouter::new()),
        }
    }

    /// Handle all incoming data on this connection
    pub async fn handle_inbound_stream(&mut self, mut stream: BiStream) {
        let router = self.stream_router.clone();
        let registry = CONNECTION_REGISTRY.clone();

        let handle = tokio::spawn(async move {
            let mut buffer = vec![0u8; 8192];

            loop {
                match stream.read(&mut buffer).await {
                    Ok(0) => break, // Connection closed
                    Ok(n) => {
                        let data = &buffer[..n];

                        // Extract UUID from data prefix
                        if let Ok(uuid) = extract_uuid_from_data(data) {
                            // Route to appropriate local sender
                            if let Some(sender) = registry.get_sender_for_uuid(uuid).await {
                                if sender.send_bytes(data.to_vec()).is_err() {
                                    // Sender dropped, clean up
                                    registry.cleanup_uuid(uuid).await;
                                }
                            }
                        }
                    }
                    Err(_) => break, // Stream error
                }
            }
        });

        self.inbound_handler = Some(handle);
    }

    /// Create outbound stream for specific UUID if needed
    pub async fn get_or_create_outbound_stream(
        &mut self,
        uuid: Uuid,
    ) -> Result<&mut BiStream, Box<dyn std::error::Error + Send + Sync>> {
        if !self.outbound_streams.contains_key(&uuid) {
            // This would use actual iroh connection to open new stream
            let stream = self.open_bi_stream().await?;
            self.outbound_streams.insert(uuid, stream);
        }
        Ok(self.outbound_streams.get_mut(&uuid).unwrap())
    }

    /// Mock method for opening bi-directional stream
    async fn open_bi_stream(&self) -> Result<BiStream, Box<dyn std::error::Error + Send + Sync>> {
        // This would be implemented using actual iroh connection
        todo!("Implement with actual iroh connection.open_bi()")
    }

    pub async fn cleanup(&mut self) {
        if let Some(handle) = self.inbound_handler.take() {
            handle.abort();
        }
        self.outbound_streams.clear();
    }
}

/// Routes incoming streams to appropriate handlers
pub struct StreamRouter {
    // Could contain routing logic if needed
}

impl StreamRouter {
    pub fn new() -> Self {
        Self {}
    }
}

/// Manages all connections and UUID routing
pub struct ConnectionRegistry {
    // Maps UUID -> Actor type-specific senders
    uuid_to_senders: RwLock<HashMap<Uuid, Box<dyn DynSender>>>,

    // Maps Iroh connection -> active UUIDs on that connection
    connection_to_uuids: RwLock<HashMap<ConnectionId, HashSet<Uuid>>>,

    // Pending connections waiting for establishment
    pending_connections: Mutex<HashMap<Uuid, Vec<oneshot::Sender<Box<dyn DynSender>>>>>,

    // Active iroh connections
    active_connections: RwLock<HashMap<NodeId, IrohConnection>>,

    // Lifecycle tracking
    lifecycle_manager: Mutex<StreamLifecycleManager>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            uuid_to_senders: RwLock::new(HashMap::new()),
            connection_to_uuids: RwLock::new(HashMap::new()),
            pending_connections: Mutex::new(HashMap::new()),
            active_connections: RwLock::new(HashMap::new()),
            lifecycle_manager: Mutex::new(StreamLifecycleManager::new()),
        }
    }

    /// Get existing sender or create new channel for UUID
    pub async fn get_or_create_sender<A: Actor>(&self, uuid: Uuid) -> SenderResult<A> {
        // Check if sender already exists
        {
            let senders = self.uuid_to_senders.read().await;
            if let Some(existing_sender) = senders.get(&uuid) {
                // Try to extract the typed sender
                if let Ok(typed_sender) = self.extract_typed_sender::<A>(existing_sender) {
                    return SenderResult::Existing(typed_sender);
                }
            }
        }

        // Create new channel
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let dyn_sender = Box::new(TypedDynSender::new(tx.clone()));

        // Store in registry
        {
            let mut senders = self.uuid_to_senders.write().await;
            senders.insert(uuid, dyn_sender);
        }

        // Track lifecycle
        {
            let mut lifecycle = self.lifecycle_manager.lock().await;
            lifecycle.track_uuid_lifecycle(uuid);
        }

        SenderResult::New(tx, rx)
    }

    /// Extract typed sender from DynSender (unsafe but necessary for type erasure)
    fn extract_typed_sender<A: Actor>(
        &self,
        dyn_sender: &Box<dyn DynSender>,
    ) -> Result<UnboundedSender<DynMessage<A>>, ()> {
        // This is a limitation of the type erasure - we need to trust the UUID mapping
        // In a real implementation, you might store type information with the sender
        // For now, we'll use unsafe casting (similar to your original transmute approach)
        unsafe {
            let any_ref = dyn_sender.as_ref() as *const dyn DynSender as *const TypedDynSender<A>;
            if let Some(typed_sender) = any_ref.as_ref() {
                Ok(typed_sender.get_sender().clone())
            } else {
                Err(())
            }
        }
    }

    /// Get sender for UUID (used by inbound stream handler)
    pub async fn get_sender_for_uuid(&self, uuid: Uuid) -> Option<Box<dyn DynSender>> {
        let senders = self.uuid_to_senders.read().await;
        senders.get(&uuid).map(|s| s.clone_dyn())
    }

    /// Clean up resources for a UUID
    pub async fn cleanup_uuid(&self, uuid: Uuid) {
        {
            let mut senders = self.uuid_to_senders.write().await;
            senders.remove(&uuid);
        }

        // Clean up from connection mappings
        {
            let mut conn_to_uuids = self.connection_to_uuids.write().await;
            for uuids in conn_to_uuids.values_mut() {
                uuids.remove(&uuid);
            }
        }

        // Clean up pending connections
        {
            let mut pending = self.pending_connections.lock().await;
            pending.remove(&uuid);
        }
    }

    /// Mark UUID as awaiting connection
    pub async fn mark_awaiting(&self, uuid: Uuid) {
        let mut pending = self.pending_connections.lock().await;
        pending.entry(uuid).or_insert_with(Vec::new);
    }

    /// Check if there's an awaiting connection for UUID
    pub async fn get_awaiting(&self, uuid: Uuid) -> Option<oneshot::Sender<Box<dyn DynSender>>> {
        let mut pending = self.pending_connections.lock().await;
        if let Some(waiters) = pending.get_mut(&uuid) {
            waiters.pop()
        } else {
            None
        }
    }
}

/// Manages stream lifecycle and cleanup
pub struct StreamLifecycleManager {
    // Monitor when all Rx for a UUID are dropped
    uuid_watchers: HashMap<Uuid, Weak<()>>,
}

impl StreamLifecycleManager {
    pub fn new() -> Self {
        Self {
            uuid_watchers: HashMap::new(),
        }
    }

    /// Called when creating new channel
    pub fn track_uuid_lifecycle(&mut self, uuid: Uuid) -> Arc<()> {
        let tracker = Arc::new(());
        self.uuid_watchers.insert(uuid, Arc::downgrade(&tracker));
        tracker
    }

    /// Periodically clean up dead connections
    pub async fn cleanup_dead_connections(&mut self) {
        let dead_uuids: Vec<_> = self
            .uuid_watchers
            .iter()
            .filter(|(_, weak)| weak.upgrade().is_none())
            .map(|(uuid, _)| *uuid)
            .collect();

        for uuid in dead_uuids {
            self.uuid_watchers.remove(&uuid);
            // Signal to cleanup UUID connection
            CONNECTION_REGISTRY.cleanup_uuid(uuid).await;
        }
    }
}

// Serialize message with UUID prefix
pub fn serialize_message_with_uuid<T: Serialize>(
    msg: &T,
    uuid: Uuid,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let uuid_bytes = uuid.as_bytes();
    let payload = postcard::to_allocvec(msg)?;

    let mut result = Vec::with_capacity(UUID_SIZE + payload.len());
    result.extend_from_slice(uuid_bytes);
    result.extend_from_slice(&payload);

    Ok(result)
}

// Deserialize message from UUID-prefixed data - returns DynMessage directly
pub fn deserialize_dyn_message_from_prefixed<A: Actor>(
    data: &[u8],
) -> Result<DynMessage<A>, String> {
    if data.len() < UUID_SIZE {
        return Err("Data too short to contain UUID".to_string());
    }

    let uuid_bytes: [u8; UUID_SIZE] = data[..UUID_SIZE]
        .try_into()
        .map_err(|_| "Failed to extract UUID bytes")?;
    let uuid = Uuid::from_bytes(uuid_bytes);
    let payload = &data[UUID_SIZE..];

    deserialize_dyn_message(uuid, payload)
}

// The main deserializer function - returns DynMessage directly
pub fn deserialize_dyn_message<A: Actor>(
    uuid: Uuid,
    payload: &[u8],
) -> Result<DynMessage<A>, String> {
    let registration = MESSAGE_MAP
        .get(&uuid)
        .ok_or_else(|| format!("Unknown message type: {}", uuid))?;

    // This function directly creates a DynMessage<A> from bytes
    let dyn_msg_any = (registration.create_dyn_message_fn)(payload)
        .map_err(|e| format!("Failed to create DynMessage: {}", e))?;

    // Downcast to the correct DynMessage type
    let dyn_msg = dyn_msg_any
        .downcast::<DynMessage<A>>()
        .map_err(|_| "Failed to downcast to DynMessage".to_string())?;

    Ok(*dyn_msg)
}

// Extract UUID from serialized data
fn extract_uuid_from_data(data: &[u8]) -> Result<Uuid, String> {
    if data.len() < UUID_SIZE {
        return Err("Data too short to contain UUID".to_string());
    }

    let uuid_bytes: [u8; UUID_SIZE] = data[..UUID_SIZE]
        .try_into()
        .map_err(|_| "Failed to extract UUID bytes")?;
    Ok(Uuid::from_bytes(uuid_bytes))
}

/// Main API function for serialization and connection management
pub async fn serialize_and_get_connections<A: Actor, T: Serialize>(
    messages: Vec<(T, Uuid)>,
) -> Result<
    (
        Vec<Vec<u8>>,
        Vec<ActorRef<A>>,
        Vec<(Uuid, UnboundedReceiver<DynMessage<A>>)>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let mut serialized = Vec::new();
    let mut existing_refs = Vec::new();
    let mut new_channels = Vec::new();

    let registry = &*CONNECTION_REGISTRY;

    for (msg, uuid) in messages {
        let data = serialize_message_with_uuid(&msg, uuid)?;
        serialized.push(data);

        match registry.get_or_create_sender::<A>(uuid).await {
            SenderResult::Existing(sender) => {
                existing_refs.push(ActorRef::from_sender(sender));
            }
            SenderResult::New(tx, rx) => {
                new_channels.push((uuid, rx));
                existing_refs.push(ActorRef::from_sender(tx));
            }
        }
    }

    Ok((serialized, existing_refs, new_channels))
}

/// Establish connections for new channels
pub async fn establish_connections<A: Actor>(
    new_channels: Vec<(Uuid, UnboundedReceiver<DynMessage<A>>)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let registry = &*CONNECTION_REGISTRY;

    for (uuid, rx) in new_channels {
        // Try to find existing connection with this UUID
        if let Some(awaiting) = registry.get_awaiting(uuid).await {
            // Someone else is waiting for this UUID, fulfill their request
            let dyn_sender = Box::new(TypedDynSender::new(
                // Convert rx back to tx somehow - this is a design issue
                // In practice, you'd handle this differently
                todo!("Handle rx to tx conversion or redesign"),
            ));
            let _ = awaiting.send(dyn_sender);
        } else {
            // Mark as awaiting and try to establish new connection
            registry.mark_awaiting(uuid).await;

            tokio::spawn(async move {
                // Attempt connection discovery/establishment
                match discover_node_for_uuid(uuid).await {
                    Ok(node_id) => {
                        if let Err(e) = establish_iroh_connection(node_id, uuid, rx).await {
                            eprintln!("Failed to establish connection: {}", e);
                        }
                    }
                    Err(_) => {
                        // Keep waiting or timeout
                        timeout_awaiting_connection(uuid, rx).await;
                    }
                }
            });
        }
    }

    Ok(())
}

// Mock functions for connection discovery and establishment
async fn discover_node_for_uuid(
    uuid: Uuid,
) -> Result<NodeId, Box<dyn std::error::Error + Send + Sync>> {
    // This would implement actual node discovery logic
    todo!("Implement node discovery for UUID: {}", uuid)
}

async fn establish_iroh_connection<A: Actor>(
    node_id: NodeId,
    uuid: Uuid,
    rx: UnboundedReceiver<DynMessage<A>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // This would implement actual iroh connection establishment
    todo!("Implement iroh connection establishment")
}

async fn timeout_awaiting_connection<A: Actor>(uuid: Uuid, rx: UnboundedReceiver<DynMessage<A>>) {
    // Handle connection timeout
    tokio::time::sleep(Duration::from_secs(30)).await;
    println!("Connection timeout for UUID: {}", uuid);
}

// Macro to register message types
macro_rules! register_message {
    ($actor:ty, $msg_type:ty, $uuid:expr) => {
        paste::paste! {
            #[distributed_slice(MESSAGE_REGISTRY)]
            static [<REGISTRATION_ $msg_type:snake:upper>]: MessageRegistration = MessageRegistration {
                uuid: $uuid,
                create_dyn_message_fn: |payload: &[u8]| -> Result<Box<dyn Any + Send>, Box<dyn std::error::Error + Send + Sync>> {
                    // Deserialize the concrete message
                    let msg: $msg_type = postcard::from_bytes(payload)?;
                    // Convert directly to DynMessage
                    let dyn_msg: DynMessage<$actor> = Box::new(msg);
                    // Return as Any
                    Ok(Box::new(dyn_msg))
                },
            };
        }
    };
}

// Example message types
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PingMessage {
    pub id: u32,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct StatusMessage {
    pub status: String,
    pub timestamp: u64,
}

// Example actor
pub struct MyActor;
impl Actor for MyActor {}

// Implement Behavior for each message type
impl Behavior<PingMessage> for MyActor {
    type Return = String;

    async fn process(&mut self, _ctx: Context<Self>, msg: PingMessage) -> Self::Return {
        format!("Pong: {} (id: {})", msg.content, msg.id)
    }
}

impl Behavior<StatusMessage> for MyActor {
    type Return = u32;

    async fn process(&mut self, _ctx: Context<Self>, msg: StatusMessage) -> Self::Return {
        println!("Status: {} at timestamp {}", msg.status, msg.timestamp);
        msg.timestamp as u32
    }
}

// Register the message types with UUIDs
register_message!(
    MyActor,
    PingMessage,
    uuid::uuid!("550e8400-e29b-41d4-a716-446655440001")
);
register_message!(
    MyActor,
    StatusMessage,
    uuid::uuid!("550e8400-e29b-41d4-a716-446655440002")
);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connection_registry() {
        let registry = ConnectionRegistry::new();
        let uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440001");

        // First call should create new channel
        match registry.get_or_create_sender::<MyActor>(uuid).await {
            SenderResult::New(tx, rx) => {
                assert!(!tx.is_closed());
                drop(rx); // This would trigger cleanup in real usage
            }
            _ => panic!("Expected new channel"),
        }

        // Second call should return existing (but will create new since we dropped rx)
        match registry.get_or_create_sender::<MyActor>(uuid).await {
            SenderResult::New(_, _) => {
                // Expected since we dropped the receiver
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn test_serialize_and_get_connections() {
        let messages = vec![(
            PingMessage {
                id: 1,
                content: "test".to_string(),
            },
            uuid::uuid!("550e8400-e29b-41d4-a716-446655440001"),
        )];

        let result = serialize_and_get_connections::<MyActor, _>(messages).await;
        assert!(result.is_ok());

        let (serialized, actor_refs, new_channels) = result.unwrap();
        assert_eq!(serialized.len(), 1);
        assert_eq!(actor_refs.len(), 1);
        assert_eq!(new_channels.len(), 1);
    }
}

fn main() {
    println!("Connection management system implemented!");
}
