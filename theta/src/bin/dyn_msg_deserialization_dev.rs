use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use std::{any::Any, collections::BTreeMap, fmt::Debug, sync::LazyLock};

// The global registry slice that linkme will populate
#[distributed_slice]
pub static MESSAGE_REGISTRY: [MessageRegistration];

// Registration entry for each message type
#[derive(Clone, Copy)]
pub struct MessageRegistration {
    pub type_id: &'static str,
    pub deserialize_fn:
        fn(&[u8]) -> Result<Box<dyn Any + Send>, Box<dyn std::error::Error + Send + Sync>>,
}

// Lazy-initialized BTreeMap for fast lookups
static MESSAGE_MAP: LazyLock<BTreeMap<&'static str, MessageRegistration>> = LazyLock::new(|| {
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

// Helper function to encode UUID string to 16 bytes
fn encode_uuid(uuid_str: &str) -> [u8; UUID_SIZE] {
    let uuid_bytes = uuid_str.replace("-", "");
    let mut result = [0u8; UUID_SIZE];

    for (i, chunk) in uuid_bytes.as_bytes().chunks(2).take(UUID_SIZE).enumerate() {
        if chunk.len() == 2 {
            if let Ok(byte) = u8::from_str_radix(&String::from_utf8_lossy(chunk), 16) {
                result[i] = byte;
            }
        }
    }
    result
}

// Helper function to decode 16 bytes back to UUID string
fn decode_uuid(bytes: &[u8; UUID_SIZE]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

// Serialize message with UUID prefix
pub fn serialize_message_with_uuid<T: Serialize>(
    msg: &T,
    uuid: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let uuid_bytes = encode_uuid(uuid);
    let payload = postcard::to_allocvec(msg)?;

    let mut result = Vec::with_capacity(UUID_SIZE + payload.len());
    result.extend_from_slice(&uuid_bytes);
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
    let uuid_str = decode_uuid(&uuid_bytes);
    let payload = &data[UUID_SIZE..];

    deserialize_message(&uuid_str, payload)
}

// The deserializer function
pub fn deserialize_message(type_id: &str, payload: &[u8]) -> Result<Box<dyn Any + Send>, String> {
    let registration = MESSAGE_MAP
        .get(type_id)
        .ok_or_else(|| format!("Unknown message type: {}", type_id))?;

    (registration.deserialize_fn)(payload).map_err(|e| format!("Deserialization failed: {}", e))
}

// Macro to register message types with unique static names
macro_rules! register_message {
    ($type:ty, $uuid:literal) => {
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
register_message!(PingMessage, "550e8400-e29b-41d4-a716-446655440001");
register_message!(StatusMessage, "550e8400-e29b-41d4-a716-446655440002");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_encoding_decoding() {
        let uuid = "550e8400-e29b-41d4-a716-446655440001";
        let encoded = encode_uuid(uuid);
        let decoded = decode_uuid(&encoded);
        assert_eq!(uuid, decoded);
        println!("✓ UUID encoding/decoding test passed");
    }

    #[test]
    fn test_message_registration() {
        // Check that messages are registered
        assert_eq!(MESSAGE_REGISTRY.len(), 2);
        assert!(MESSAGE_MAP.contains_key("550e8400-e29b-41d4-a716-446655440001"));
        assert!(MESSAGE_MAP.contains_key("550e8400-e29b-41d4-a716-446655440002"));
    }

    #[test]
    fn test_ping_message_with_uuid_prefix() {
        let original = PingMessage {
            id: 123,
            content: "Hello World".to_string(),
        };

        // Serialize with UUID prefix
        let serialized =
            serialize_message_with_uuid(&original, "550e8400-e29b-41d4-a716-446655440001").unwrap();

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

        // Serialize with UUID prefix
        let serialized =
            serialize_message_with_uuid(&original, "550e8400-e29b-41d4-a716-446655440002").unwrap();

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

        // Serialize just the payload
        let payload = postcard::to_allocvec(&original).unwrap();

        // Deserialize using our system with separate UUID
        let deserialized_any =
            deserialize_message("550e8400-e29b-41d4-a716-446655440001", &payload).unwrap();

        let deserialized = deserialized_any.downcast::<PingMessage>().unwrap();

        assert_eq!(*deserialized, original);
        println!("✓ Separate serialization test passed");
    }

    #[test]
    fn test_unknown_message_type() {
        let result = deserialize_message("unknown-uuid", &[1, 2, 3]);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown message type"));
        println!("✓ Unknown message type test passed");
    }

    #[test]
    fn test_invalid_payload() {
        let result = deserialize_message(
            "550e8400-e29b-41d4-a716-446655440001",
            &[255, 255, 255], // Invalid postcard data
        );

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

    // Method 1: Serialize with UUID prefix (all-in-one binary)
    let ping_prefixed =
        serialize_message_with_uuid(&ping, "550e8400-e29b-41d4-a716-446655440001").unwrap();

    let status_prefixed =
        serialize_message_with_uuid(&status, "550e8400-e29b-41d4-a716-446655440002").unwrap();

    // Deserialize from prefixed data
    let ping_restored = deserialize_message_from_prefixed(&ping_prefixed).unwrap();
    let status_restored = deserialize_message_from_prefixed(&status_prefixed).unwrap();

    // Method 2: Separate UUID and payload
    let ping_payload = postcard::to_allocvec(&ping).unwrap();
    let ping_separate =
        deserialize_message("550e8400-e29b-41d4-a716-446655440001", &ping_payload).unwrap();

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
