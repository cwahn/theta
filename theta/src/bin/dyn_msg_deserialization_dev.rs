use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use std::{any::Any, collections::BTreeMap, fmt::Debug, sync::LazyLock};
use uuid::Uuid;

// The global registry slice that linkme will populate
#[distributed_slice]
pub static MESSAGE_REGISTRY: [MessageRegistration];

// Registration entry for each message type
#[derive(Clone, Copy)]
pub struct MessageRegistration {
    pub type_id: Uuid,
    pub deserialize_fn:
        fn(&[u8]) -> Result<Box<dyn Any + Send>, Box<dyn std::error::Error + Send + Sync>>,
}

// Lazy-initialized BTreeMap for fast lookups
static MESSAGE_MAP: LazyLock<BTreeMap<Uuid, MessageRegistration>> = LazyLock::new(|| {
    MESSAGE_REGISTRY
        .iter()
        .map(|reg| (reg.type_id, *reg))
        .collect()
});

// Dynamic message trait - your existing trait
pub trait Message<A>: Debug + Send
where
    A: Actor,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<A>,
        state: &'a mut A,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Box<dyn Any + Send>> + Send + 'a>>;
}

// Placeholder types for your existing system
pub trait Actor: Send + 'static {}
pub struct Context<A>(std::marker::PhantomData<A>);

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

// Deserialize message from UUID-prefixed data
pub fn deserialize_message_from_prefixed(data: &[u8]) -> Result<Box<dyn Any + Send>, String> {
    if data.len() < UUID_SIZE {
        return Err("Data too short to contain UUID".to_string());
    }

    let uuid_bytes: [u8; UUID_SIZE] = data[..UUID_SIZE]
        .try_into()
        .map_err(|_| "Failed to extract UUID bytes")?;
    let uuid = Uuid::from_bytes(uuid_bytes);
    let payload = &data[UUID_SIZE..];

    deserialize_message(uuid, payload)
}

// The deserializer function
pub fn deserialize_message(type_id: Uuid, payload: &[u8]) -> Result<Box<dyn Any + Send>, String> {
    let registration = MESSAGE_MAP
        .get(&type_id)
        .ok_or_else(|| format!("Unknown message type: {}", type_id))?;

    (registration.deserialize_fn)(payload).map_err(|e| format!("Deserialization failed: {}", e))
}

// Macro to register message types with unique static names
macro_rules! register_message {
    ($type:ty, $uuid:expr) => {
        paste::paste! {
            #[distributed_slice(MESSAGE_REGISTRY)]
            static [<REGISTRATION_ $type:snake:upper>]: MessageRegistration = MessageRegistration {
                type_id: $uuid,
                deserialize_fn: |payload: &[u8]| -> Result<Box<dyn Any + Send>, Box<dyn std::error::Error + Send + Sync>> {
                    let msg: $type = postcard::from_bytes(payload)?;
                    Ok(Box::new(msg))
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

// Implement Message trait for our types
impl Message<MyActor> for PingMessage {
    fn process_dyn<'a>(
        self: Box<Self>,
        _ctx: Context<MyActor>,
        _state: &'a mut MyActor,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Box<dyn Any + Send>> + Send + 'a>> {
        Box::pin(async move {
            println!("Processing PingMessage: {:?}", self);
            Box::new(format!("Pong: {}", self.content)) as Box<dyn Any + Send>
        })
    }
}

impl Message<MyActor> for StatusMessage {
    fn process_dyn<'a>(
        self: Box<Self>,
        _ctx: Context<MyActor>,
        _state: &'a mut MyActor,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Box<dyn Any + Send>> + Send + 'a>> {
        Box::pin(async move {
            println!("Processing StatusMessage: {:?}", self);
            Box::new(42u32) as Box<dyn Any + Send>
        })
    }
}

// Register the message types with UUIDs
register_message!(
    PingMessage,
    uuid::uuid!("550e8400-e29b-41d4-a716-446655440001")
);
register_message!(
    StatusMessage,
    uuid::uuid!("550e8400-e29b-41d4-a716-446655440002")
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_registration() {
        // Check that messages are registered
        assert_eq!(MESSAGE_REGISTRY.len(), 2);
        assert!(MESSAGE_MAP.contains_key(&uuid::uuid!("550e8400-e29b-41d4-a716-446655440001")));
        assert!(MESSAGE_MAP.contains_key(&uuid::uuid!("550e8400-e29b-41d4-a716-446655440002")));
    }

    #[test]
    fn test_ping_message_with_uuid_prefix() {
        let original = PingMessage {
            id: 123,
            content: "Hello World".to_string(),
        };

        let uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440001");

        // Serialize with UUID prefix
        let serialized = serialize_message_with_uuid(&original, uuid).unwrap();

        // Deserialize from prefixed data
        let deserialized_any = deserialize_message_from_prefixed(&serialized).unwrap();

        // Downcast back to original type
        let deserialized = deserialized_any.downcast::<PingMessage>().unwrap();

        assert_eq!(*deserialized, original);
        println!("✓ PingMessage with UUID prefix test passed");
    }

    #[test]
    fn test_status_message_with_uuid_prefix() {
        let original = StatusMessage {
            status: "Running".to_string(),
            timestamp: 1234567890,
        };

        let uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440002");

        // Serialize with UUID prefix
        let serialized = serialize_message_with_uuid(&original, uuid).unwrap();

        // Deserialize from prefixed data
        let deserialized_any = deserialize_message_from_prefixed(&serialized).unwrap();

        // Downcast back to original type
        let deserialized = deserialized_any.downcast::<StatusMessage>().unwrap();

        assert_eq!(*deserialized, original);
        println!("✓ StatusMessage with UUID prefix test passed");
    }

    #[test]
    fn test_separate_serialization() {
        let original = PingMessage {
            id: 456,
            content: "Test".to_string(),
        };

        let uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440001");

        // Serialize just the payload
        let payload = postcard::to_allocvec(&original).unwrap();

        // Deserialize using our system with separate UUID
        let deserialized_any = deserialize_message(uuid, &payload).unwrap();

        let deserialized = deserialized_any.downcast::<PingMessage>().unwrap();

        assert_eq!(*deserialized, original);
        println!("✓ Separate serialization test passed");
    }

    #[test]
    fn test_unknown_message_type() {
        let unknown_uuid = uuid::uuid!("00000000-0000-0000-0000-000000000000");
        let result = deserialize_message(unknown_uuid, &[1, 2, 3]);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown message type"));
        println!("✓ Unknown message type test passed");
    }

    #[test]
    fn test_invalid_payload() {
        let uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440001");
        let result = deserialize_message(uuid, &[255, 255, 255]); // Invalid postcard data

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Deserialization failed"));
        println!("✓ Invalid payload test passed");
    }

    #[test]
    fn test_short_data() {
        let result = deserialize_message_from_prefixed(&[1, 2, 3]); // Too short for UUID
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Data too short"));
        println!("✓ Short data test passed");
    }

    #[tokio::test]
    async fn test_message_processing() {
        let ping = PingMessage {
            id: 42,
            content: "test".to_string(),
        };

        let mut actor = MyActor;
        let ctx = Context(std::marker::PhantomData);

        // Process the message using the trait method
        let boxed_ping: Box<dyn Message<MyActor>> = Box::new(ping);
        let result = boxed_ping.process_dyn(ctx, &mut actor).await;

        // Downcast the result
        let response = result.downcast::<String>().unwrap();
        assert_eq!(*response, "Pong: test");
        println!("✓ Message processing test passed");
    }
}

// Example usage function
pub fn example_usage() {
    println!("=== Dynamic Message System Example ===");

    // Create messages
    let ping = PingMessage {
        id: 1,
        content: "Hello".to_string(),
    };
    let status = StatusMessage {
        status: "OK".to_string(),
        timestamp: 12345,
    };

    let ping_uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440001");
    let status_uuid = uuid::uuid!("550e8400-e29b-41d4-a716-446655440002");

    // Method 1: Serialize with UUID prefix (all-in-one binary)
    let ping_prefixed = serialize_message_with_uuid(&ping, ping_uuid).unwrap();
    let status_prefixed = serialize_message_with_uuid(&status, status_uuid).unwrap();

    // Deserialize from prefixed data
    let ping_restored = deserialize_message_from_prefixed(&ping_prefixed).unwrap();
    let status_restored = deserialize_message_from_prefixed(&status_prefixed).unwrap();

    // Method 2: Separate UUID and payload
    let ping_payload = postcard::to_allocvec(&ping).unwrap();
    let ping_separate = deserialize_message(ping_uuid, &ping_payload).unwrap();

    // Verify they match
    let ping_typed1 = ping_restored.downcast::<PingMessage>().unwrap();
    let ping_typed2 = ping_separate.downcast::<PingMessage>().unwrap();
    let status_typed = status_restored.downcast::<StatusMessage>().unwrap();

    println!("Original ping: {:?}", ping);
    println!("Restored ping (prefixed): {:?}", ping_typed1);
    println!("Restored ping (separate): {:?}", ping_typed2);
    println!("Original status: {:?}", status);
    println!("Restored status: {:?}", status_typed);

    assert_eq!(*ping_typed1, ping);
    assert_eq!(*ping_typed2, ping);
    assert_eq!(*status_typed, status);

    println!("✓ All tests passed!");
}

fn main() {
    example_usage();
    println!("Program completed successfully!");
}
