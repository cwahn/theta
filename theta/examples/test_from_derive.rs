//! Simple test to verify ActorArgs derive generates From<&Self>

use serde::{Deserialize, Serialize};
use theta::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
struct TestActor {
    value: i32,
    name: String,
}

#[actor("12345678-1234-5678-9abc-123456789abc")]
impl Actor for TestActor {
    type View = Nil;
    const _: () = {};
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
