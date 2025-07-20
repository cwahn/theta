use futures::{FutureExt, future::BoxFuture};
use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use std::{any::Any, collections::BTreeMap, fmt::Debug, sync::LazyLock};
use uuid::Uuid;

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

// Core traits
pub trait Actor: Send + 'static {}

pub struct Context<A>(std::marker::PhantomData<A>);

pub type DynMessage<A> = Box<dyn Message<A>>;

pub trait Behavior<M: Send + 'static>: Actor + Sized {
    type Return: Debug + Send + 'static;
    fn process(
        &mut self,
        ctx: Context<Self>,
        msg: M,
    ) -> impl std::future::Future<Output = Self::Return> + Send;
}

pub trait Message<A>: Debug + Send
where
    A: Actor,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send>>;
}

// Implementations
impl<A, M> Message<A> for M
where
    A: Actor + Behavior<M>,
    M: Debug + Send + 'static,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send>> {
        async move {
            let res = state.process(ctx, *self).await;
            Box::new(res) as Box<dyn Any + Send>
        }
        .boxed()
    }
}

// UUID size (16 bytes)
const UUID_SIZE: usize = 16;

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

// Macro to register message types - directly creates DynMessage
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ComputeMessage {
    pub operation: String,
    pub values: Vec<i32>,
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

impl Behavior<ComputeMessage> for MyActor {
    type Return = i32;

    async fn process(&mut self, _ctx: Context<Self>, msg: ComputeMessage) -> Self::Return {
        match msg.operation.as_str() {
            "sum" => msg.values.iter().sum(),
            "product" => msg.values.iter().product(),
            _ => 0,
        }
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
register_message!(
    MyActor,
    ComputeMessage,
    uuid::uuid!("550e8400-e29b-41d4-a716-446655440003")
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_registration() {
        assert_eq!(MESSAGE_REGISTRY.len(), 3);
        assert!(MESSAGE_MAP.contains_key(&uuid::uuid!("550e8400-e29b-41d4-a716-446655440001")));
        assert!(MESSAGE_MAP.contains_key(&uuid::uuid!("550e8400-e29b-41d4-a716-446655440002")));
        assert!(MESSAGE_MAP.contains_key(&uuid::uuid!("550e8400-e29b-41d4-a716-446655440003")));
    }

    #[tokio::test]
    async fn test_dyn_message_processing_from_bytes_only() {
        let mut actor = MyActor;
        // let ctx: Context<MyActor> = Context(std::marker::PhantomData);

        // Simulate what happens in real usage: we only have serialized bytes from different sources

        // Some system serialized a PingMessage (we don't know this on receiving side)
        let ping_bytes = {
            let ping = PingMessage {
                id: 42,
                content: "Hello".to_string(),
            };
            serialize_message_with_uuid(&ping, uuid::uuid!("550e8400-e29b-41d4-a716-446655440001"))
                .unwrap()
        };

        // Some system serialized a StatusMessage (we don't know this on receiving side)
        let status_bytes = {
            let status = StatusMessage {
                status: "Running".to_string(),
                timestamp: 1000,
            };
            serialize_message_with_uuid(
                &status,
                uuid::uuid!("550e8400-e29b-41d4-a716-446655440002"),
            )
            .unwrap()
        };

        // Some system serialized a ComputeMessage (we don't know this on receiving side)
        let compute_bytes = {
            let compute = ComputeMessage {
                operation: "sum".to_string(),
                values: vec![1, 2, 3],
            };
            serialize_message_with_uuid(
                &compute,
                uuid::uuid!("550e8400-e29b-41d4-a716-446655440003"),
            )
            .unwrap()
        };

        // Now on the receiving side, we only have bytes and must process them
        let message_bytes_list = vec![ping_bytes, status_bytes, compute_bytes];
        let mut results = Vec::new();

        for bytes in message_bytes_list {
            // This is the only thing we can do - deserialize bytes to DynMessage and process
            let dyn_msg: DynMessage<MyActor> =
                deserialize_dyn_message_from_prefixed(&bytes).unwrap();
            let result = dyn_msg
                .process_dyn(Context(std::marker::PhantomData), &mut actor)
                .await;
            results.push(result);
        }

        // Verify results
        assert_eq!(results.len(), 3);

        let ping_response = results[0].downcast_ref::<String>().unwrap();
        assert_eq!(ping_response, "Pong: Hello (id: 42)");

        let status_response = results[1].downcast_ref::<u32>().unwrap();
        assert_eq!(*status_response, 1000);

        let compute_response = results[2].downcast_ref::<i32>().unwrap();
        assert_eq!(*compute_response, 6); // 1+2+3 = 6

        println!("✓ All dynamic message processing from bytes only passed!");
    }

    #[tokio::test]
    async fn test_message_queue_simulation() {
        let mut actor = MyActor;
        // let ctx = Context(std::marker::PhantomData);

        // Simulate a message queue where we only receive Vec<u8>
        let incoming_messages: Vec<Vec<u8>> = vec![
            // These would come from different senders/systems
            serialize_message_with_uuid(
                &PingMessage {
                    id: 1,
                    content: "First".to_string(),
                },
                uuid::uuid!("550e8400-e29b-41d4-a716-446655440001"),
            )
            .unwrap(),
            serialize_message_with_uuid(
                &ComputeMessage {
                    operation: "product".to_string(),
                    values: vec![2, 3],
                },
                uuid::uuid!("550e8400-e29b-41d4-a716-446655440003"),
            )
            .unwrap(),
            serialize_message_with_uuid(
                &StatusMessage {
                    status: "OK".to_string(),
                    timestamp: 500,
                },
                uuid::uuid!("550e8400-e29b-41d4-a716-446655440002"),
            )
            .unwrap(),
            serialize_message_with_uuid(
                &PingMessage {
                    id: 2,
                    content: "Last".to_string(),
                },
                uuid::uuid!("550e8400-e29b-41d4-a716-446655440001"),
            )
            .unwrap(),
        ];

        // Process the message queue - this is exactly what would happen in real usage
        let mut processed_results = Vec::new();

        for message_bytes in incoming_messages {
            // The only thing we know is that we have bytes that should become a DynMessage
            let dyn_msg: DynMessage<MyActor> =
                deserialize_dyn_message_from_prefixed(&message_bytes).unwrap();

            // Process the message without knowing its concrete type
            let result = dyn_msg
                .process_dyn(Context(std::marker::PhantomData), &mut actor)
                .await;
            processed_results.push(result);
        }

        // Verify the processing worked correctly
        assert_eq!(processed_results.len(), 4);

        assert_eq!(
            processed_results[0].downcast_ref::<String>().unwrap(),
            "Pong: First (id: 1)"
        );
        assert_eq!(*processed_results[1].downcast_ref::<i32>().unwrap(), 6); // 2*3
        assert_eq!(*processed_results[2].downcast_ref::<u32>().unwrap(), 500);
        assert_eq!(
            processed_results[3].downcast_ref::<String>().unwrap(),
            "Pong: Last (id: 2)"
        );

        println!("✓ Message queue simulation passed!");
    }
}

// Example usage function
pub fn example_usage() {
    println!("=== Dynamic Message System Example ===");

    // This simulates the sending side
    let ping = PingMessage {
        id: 1,
        content: "Hello".to_string(),
    };
    let serialized =
        serialize_message_with_uuid(&ping, uuid::uuid!("550e8400-e29b-41d4-a716-446655440001"))
            .unwrap();
    println!("Serialized {} bytes", serialized.len());

    // This simulates the receiving side - only bytes are available
    let _dyn_msg: DynMessage<MyActor> = deserialize_dyn_message_from_prefixed(&serialized).unwrap();
    println!("Deserialized to DynMessage successfully");

    println!("✓ Example completed!");
}

fn main() {
    example_usage();
    println!("Program completed successfully!");
}
