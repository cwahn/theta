//! Simple test to verify ActorArgs derive generates From<&Self>

use theta::prelude::*;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
struct TestActor {
    value: i32,
    name: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Test that the automatic From<&Self> implementation works
    let actor = TestActor {
        value: 42,
        name: "test".to_string(),
    };
    
    // This should work thanks to the automatic From implementation
    let cloned_actor: TestActor = From::from(&actor);
    
    println!("Original: {:?}", actor);
    println!("Cloned: {:?}", cloned_actor);
    
    assert_eq!(actor.value, cloned_actor.value);
    assert_eq!(actor.name, cloned_actor.name);
    
    println!("✅ Automatic From<&Self> implementation works correctly!");
    
    Ok(())
}
