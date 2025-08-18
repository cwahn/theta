use theta::prelude::*;
use serde::{Deserialize, Serialize};

// Example 1: Actor with explicit snapshot type
#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct Counter {
    pub count: i32,
}

impl From<&Counter> for Counter {
    fn from(actor: &Counter) -> Self {
        Self { count: actor.count }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Increment(pub i32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCount;

#[actor("847d1a75-bf42-4690-b947-c3f206fda4cf", snapshot = Counter)]
impl Actor for Counter {
    const _: () = {
        async |Increment(value): Increment| {
            self.count += value;
        };

        async |_: GetCount| -> i32 { self.count };
    };
}

// Example 2: Actor with default snapshot (Self)
#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct Calculator {
    pub result: f64,
}

impl From<&Calculator> for Calculator {
    fn from(actor: &Calculator) -> Self {
        Self { result: actor.result }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Add(pub f64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResult;

#[actor("947d1a75-bf42-4690-b947-c3f206fda4cf", snapshot)]
impl Actor for Calculator {
    const _: () = {
        async |Add(value): Add| {
            self.result += value;
        };

        async |_: GetResult| -> f64 { self.result };
    };
}

// Example 3: Regular non-persistent actor
#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct SimpleActor {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMessage(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMessage;

#[actor("a47d1a75-bf42-4690-b947-c3f206fda4cf")]
impl Actor for SimpleActor {
    const _: () = {
        async |SetMessage(msg): SetMessage| {
            self.message = msg;
        };

        async |_: GetMessage| -> String { self.message.clone() };
    };
}

#[tokio::test]
async fn test_all_actor_types() {
    let root_ctx = RootContext::init_local();

    // Test persistent counter with explicit snapshot type
    let counter = root_ctx.spawn(Counter { count: 0 });
    counter.tell(Increment(5)).unwrap();
    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 5);

    // Test persistent calculator with default snapshot type (Self)
    let calc = root_ctx.spawn(Calculator { result: 0.0 });
    calc.tell(Add(10.5)).unwrap();
    let result = calc.ask(GetResult).await.unwrap();
    assert_eq!(result, 10.5);

    // Test regular non-persistent actor
    let simple = root_ctx.spawn(SimpleActor { message: "Hello".to_string() });
    simple.tell(SetMessage("World".to_string())).unwrap();
    let message = simple.ask(GetMessage).await.unwrap();
    assert_eq!(message, "World");
}
