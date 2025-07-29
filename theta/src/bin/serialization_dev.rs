use serde::{Serialize, Serializer};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;
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

// Task-local storage for collecting senders
task_local! {
    static SENDER_COLLECTOR: RefCell<Option<HashMap<Uuid, Box<dyn Any + Send>>>>;
}

// Implement Serialize for ActorRef
impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Try to collect the sender in task-local storage
        let _ = SENDER_COLLECTOR.try_with(|collector| {
            if let Some(ref mut map) = *collector.borrow_mut() {
                map.insert(self.0, Box::new(self.1.clone()));
            }
        });

        // Serialize only the UUID
        self.0.serialize(serializer)
    }
}

// Collecting serialization function
pub async fn serialize_with_collection<T: Serialize>(
    value: &T,
) -> Result<(Vec<u8>, HashMap<Uuid, Box<dyn Any + Send>>), serde_json::Error> {
    SENDER_COLLECTOR
        .scope(RefCell::new(Some(HashMap::new())), async {
            // Serialize the value
            let serialized = serde_json::to_vec(value)?;

            // Extract collected senders
            let collected = SENDER_COLLECTOR
                .with(|collector| collector.borrow_mut().take().unwrap_or_default());

            Ok((serialized, collected))
        })
        .await
}

// Test types
#[derive(Debug)]
pub struct TestActor;
impl Actor for TestActor {}

#[derive(Serialize, Debug)]
pub struct TestData {
    pub name: String,
    pub value: i32,
    pub actor: ActorRef<TestActor>,
    pub actors: Vec<ActorRef<TestActor>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_actor_ref_serialization() {
        // Create some actor references
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        let (tx3, _rx3) = mpsc::unbounded_channel();

        let actor_ref1 = ActorRef::<TestActor>(Uuid::new_v4(), tx1);
        let actor_ref2 = ActorRef::<TestActor>(Uuid::new_v4(), tx2);
        let actor_ref3 = ActorRef::<TestActor>(Uuid::new_v4(), tx3);

        let test_data = TestData {
            name: "test_struct".to_string(),
            value: 42,
            actor: actor_ref1.clone(),
            actors: vec![actor_ref2.clone(), actor_ref3.clone()],
        };

        // Serialize with collection
        let (serialized_bytes, collected_senders) = serialize_with_collection(&test_data)
            .await
            .expect("Serialization should succeed");

        // Verify we collected 3 unique senders (1 single + 2 in vec)
        assert_eq!(collected_senders.len(), 3);
        assert!(collected_senders.contains_key(&actor_ref1.0));
        assert!(collected_senders.contains_key(&actor_ref2.0));
        assert!(collected_senders.contains_key(&actor_ref3.0));

        // Verify the serialized data contains only UUIDs
        let serialized_json = String::from_utf8(serialized_bytes).unwrap();
        println!("Serialized JSON: {serialized_json}");

        // The JSON should contain the UUIDs but not the actual senders
        assert!(serialized_json.contains(&actor_ref1.0.to_string()));
        assert!(serialized_json.contains(&actor_ref2.0.to_string()));
        assert!(serialized_json.contains(&actor_ref3.0.to_string()));

        // Verify we can deserialize back (this will only give us UUIDs)
        let deserialized: serde_json::Value = serde_json::from_str(&serialized_json).unwrap();
        println!("Deserialized: {deserialized:#}");
    }

    #[tokio::test]
    async fn test_multiple_tasks_isolation() {
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        let actor1 = ActorRef::<TestActor>(Uuid::new_v4(), tx1);
        let actor2 = ActorRef::<TestActor>(Uuid::new_v4(), tx2);

        // Task 1
        let task1 = {
            let actor1 = actor1.clone();
            tokio::spawn(async move {
                let data = TestData {
                    name: "task1".to_string(),
                    value: 1,
                    actor: actor1,
                    actors: vec![],
                };
                serialize_with_collection(&data).await
            })
        };

        // Task 2
        let task2 = {
            let actor2 = actor2.clone();
            tokio::spawn(async move {
                let data = TestData {
                    name: "task2".to_string(),
                    value: 2,
                    actor: actor2,
                    actors: vec![],
                };
                serialize_with_collection(&data).await
            })
        };

        let (result1, result2) = tokio::join!(task1, task2);
        let (_, senders1) = result1.unwrap().unwrap();
        let (_, senders2) = result2.unwrap().unwrap();

        // Each task should have collected only its own sender
        assert_eq!(senders1.len(), 1);
        assert_eq!(senders2.len(), 1);
        assert!(senders1.contains_key(&actor1.0));
        assert!(senders2.contains_key(&actor2.0));
    }
}

// Example usage
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create some actor references
    let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();

    let actor_ref = ActorRef::<TestActor>(Uuid::new_v4(), tx1);
    let actor_ref2 = ActorRef::<TestActor>(Uuid::new_v4(), tx2);

    let test_data = TestData {
        name: "example".to_string(),
        value: 123,
        actor: actor_ref.clone(),
        actors: vec![actor_ref2.clone()],
    };

    // Serialize with collection
    let (serialized_bytes, collected_senders) = serialize_with_collection(&test_data).await?;

    println!("Serialized {} bytes", serialized_bytes.len());
    println!("Collected {} senders:", collected_senders.len());
    for uuid in collected_senders.keys() {
        println!("  - {uuid}");
    }

    // Show the JSON output
    let json_str = String::from_utf8(serialized_bytes)?;
    println!("\nSerialized JSON:\n{json_str}");

    Ok(())
}
