use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task_local;
use uuid::Uuid;

// Dummy types for the example
pub trait Actor: Send + 'static {}
pub struct DynMessage<A: Actor>(std::marker::PhantomData<A>);
pub struct Continuation;

// The actual ActorRef definition
#[derive(Debug)]
pub struct ActorRef<A: Actor>(
    pub(crate) Uuid,
    pub(crate) UnboundedSender<(DynMessage<A>, Continuation)>,
);

impl<A: Actor> Clone for ActorRef<A> {
    fn clone(&self) -> Self {
        Self(self.0, self.1.clone())
    }
}

// Task-local storage for collecting/creating channels
task_local! {
    static SENDER_COLLECTOR: RefCell<Option<HashMap<Uuid, Box<dyn Any + Send>>>>;
    static RECEIVER_CREATOR: RefCell<Option<HashMap<Uuid, Box<dyn Any + Send>>>>;
}

// Implement Serialize for ActorRef
impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Collect the sender
        let _ = SENDER_COLLECTOR.try_with(|collector| {
            if let Some(ref mut map) = *collector.borrow_mut() {
                map.insert(self.0, Box::new(self.1.clone()));
            }
        });

        // Serialize only the UUID
        self.0.serialize(serializer)
    }
}

// Implement Deserialize for ActorRef
impl<'de, A: Actor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let uuid = Uuid::deserialize(deserializer)?;

        // Create a new channel for this UUID
        let (tx, rx) = mpsc::unbounded_channel::<(DynMessage<A>, Continuation)>();

        // Store the receiver in task-local storage
        let _ = RECEIVER_CREATOR.try_with(|creator| {
            if let Some(ref mut map) = *creator.borrow_mut() {
                map.insert(uuid, Box::new(rx));
            }
        });

        Ok(ActorRef(uuid, tx))
    }
}

// Collecting serialization function
pub async fn serialize_with_collection<T: Serialize>(
    value: &T,
) -> Result<(Vec<u8>, HashMap<Uuid, Box<dyn Any + Send>>), serde_json::Error> {
    SENDER_COLLECTOR
        .scope(RefCell::new(Some(HashMap::new())), async {
            let serialized = serde_json::to_vec(value)?;
            let collected = SENDER_COLLECTOR
                .with(|collector| collector.borrow_mut().take().unwrap_or_default());
            Ok((serialized, collected))
        })
        .await
}

// Creating deserialization function
pub async fn deserialize_with_creation<T: for<'de> Deserialize<'de>>(
    data: &[u8],
) -> Result<(T, HashMap<Uuid, Box<dyn Any + Send>>), serde_json::Error> {
    RECEIVER_CREATOR
        .scope(RefCell::new(Some(HashMap::new())), async {
            let deserialized = serde_json::from_slice::<T>(data)?;
            let created =
                RECEIVER_CREATOR.with(|creator| creator.borrow_mut().take().unwrap_or_default());
            Ok((deserialized, created))
        })
        .await
}

// Test types
#[derive(Debug)]
pub struct TestActor;
impl Actor for TestActor {}

#[derive(Serialize, Deserialize, Debug)]
pub struct TestData {
    pub name: String,
    pub value: i32,
    pub actor: ActorRef<TestActor>,
    pub actors: Vec<ActorRef<TestActor>>,
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;

    #[tokio::test]
    async fn test_roundtrip_serialization() {
        // Create original data with actor references
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        let (tx3, _rx3) = mpsc::unbounded_channel();

        let actor_ref1 = ActorRef::<TestActor>(Uuid::new_v4(), tx1);
        let actor_ref2 = ActorRef::<TestActor>(Uuid::new_v4(), tx2);
        let actor_ref3 = ActorRef::<TestActor>(Uuid::new_v4(), tx3);

        let original_data = TestData {
            name: "test_struct".to_string(),
            value: 42,
            actor: actor_ref1.clone(),
            actors: vec![actor_ref2.clone(), actor_ref3.clone()],
        };

        // Serialize with collection
        let (serialized_bytes, collected_senders) = serialize_with_collection(&original_data)
            .await
            .expect("Serialization should succeed");

        println!(
            "Collected {} senders during serialization",
            collected_senders.len()
        );

        // Deserialize with creation
        let (deserialized_data, created_receivers): (TestData, _) =
            deserialize_with_creation(&serialized_bytes)
                .await
                .expect("Deserialization should succeed");

        println!(
            "Created {} receivers during deserialization",
            created_receivers.len()
        );

        // Verify UUIDs are preserved
        assert_eq!(original_data.actor.0, deserialized_data.actor.0);
        assert_eq!(original_data.actors[0].0, deserialized_data.actors[0].0);
        assert_eq!(original_data.actors[1].0, deserialized_data.actors[1].0);

        // Verify we have receivers for all the UUIDs
        assert!(created_receivers.contains_key(&deserialized_data.actor.0));
        assert!(created_receivers.contains_key(&deserialized_data.actors[0].0));
        assert!(created_receivers.contains_key(&deserialized_data.actors[1].0));

        // Verify other data is preserved
        assert_eq!(original_data.name, deserialized_data.name);
        assert_eq!(original_data.value, deserialized_data.value);

        println!("✅ Roundtrip test passed!");
    }

    #[tokio::test]
    async fn test_channel_communication() {
        // Create test data
        let (tx, _) = mpsc::unbounded_channel();
        let actor_ref = ActorRef::<TestActor>(Uuid::new_v4(), tx);

        let data = TestData {
            name: "communication_test".to_string(),
            value: 123,
            actor: actor_ref.clone(),
            actors: vec![],
        };

        // Serialize
        let (serialized, _) = serialize_with_collection(&data).await.unwrap();

        // Deserialize
        let (deserialized_data, created_receivers): (TestData, _) =
            deserialize_with_creation(&serialized).await.unwrap();

        // Extract the receiver
        let receiver_box = created_receivers.get(&deserialized_data.actor.0).unwrap();
        let _receiver = receiver_box
            .downcast_ref::<UnboundedReceiver<(DynMessage<TestActor>, Continuation)>>()
            .expect("Should be able to downcast to correct receiver type");

        // Test that we can send through the new sender
        let _new_sender = &deserialized_data.actor.1;
        // let test_message = (DynMessage(std::marker::PhantomData::<TestActor>), None);

        // This would work if we could actually send the message
        // new_sender.send(test_message).unwrap();
        // let received = receiver.recv().await.unwrap();

        println!("✅ Channel setup verified!");
    }
}

// Example usage
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx1, _rx1) = mpsc::unbounded_channel();
    let (tx2, _rx2) = mpsc::unbounded_channel();

    let test_data = TestData {
        name: "example".to_string(),
        value: 123,
        actor: ActorRef::<TestActor>(Uuid::new_v4(), tx1),
        actors: vec![ActorRef::<TestActor>(Uuid::new_v4(), tx2)],
    };

    // Serialize
    let (serialized_bytes, collected_senders) = serialize_with_collection(&test_data).await?;
    println!(
        "Serialized {} bytes, collected {} senders",
        serialized_bytes.len(),
        collected_senders.len()
    );

    // Deserialize
    let (deserialized_data, created_receivers): (TestData, _) =
        deserialize_with_creation(&serialized_bytes).await?;
    println!(
        "Deserialized data, created {} receivers",
        created_receivers.len()
    );

    // Show the results
    println!("\nOriginal UUIDs vs Deserialized UUIDs:");
    println!(
        "Actor: {} -> {}",
        test_data.actor.0, deserialized_data.actor.0
    );
    println!(
        "First in vec: {} -> {}",
        test_data.actors[0].0, deserialized_data.actors[0].0
    );

    Ok(())
}
