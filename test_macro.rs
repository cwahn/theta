use theta::prelude::*;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, ActorArgs)]
struct TestActor;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestMessage;

#[actor("12345678-1234-5678-9abc-123456789abc")]
impl Actor for TestActor {
    const _: () = {
        async |_: TestMessage| {
            println!("Message received!");
        };
    };
}

fn main() {
    println!("Macro test compiled successfully!");
}
