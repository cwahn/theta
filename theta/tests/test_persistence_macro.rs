use theta::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct PersistentCounter {
    pub count: i32,
}

impl From<&PersistentCounter> for PersistentCounter {
    fn from(actor: &PersistentCounter) -> Self {
        Self { count: actor.count }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCount;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Increment(pub i32);

#[actor("847d1a75-bf42-4690-b947-c3f206fda4cf", snapshot = PersistentCounter)]
impl Actor for PersistentCounter {
    const _: () = {
        async |_: GetCount| -> i32 { self.count };
        async |Increment(value): Increment| {
            self.count += value;
        };
    };
}

#[tokio::test]
async fn test_persistent_actor_macro() {
    let root_ctx = RootContext::init_local();
    let actor = root_ctx.spawn(PersistentCounter { count: 42 });
    
    let result = actor.ask(GetCount).await.unwrap();
    assert_eq!(result, 42);
    
    actor.tell(Increment(10)).unwrap();
    
    let result = actor.ask(GetCount).await.unwrap();
    assert_eq!(result, 52);
}

#[tokio::test]
async fn test_persistent_actor_default_snapshot() {
    use theta::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
    pub struct DefaultSnapshot {
        pub value: String,
    }

    impl From<&DefaultSnapshot> for DefaultSnapshot {
        fn from(actor: &DefaultSnapshot) -> Self {
            Self { value: actor.value.clone() }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GetValue;

    #[actor("147d1a75-bf42-4690-b947-c3f206fda4cf", snapshot)]
    impl Actor for DefaultSnapshot {
        const _: () = {
            async |_: GetValue| -> String { self.value.clone() };
        };
    }

    let root_ctx = RootContext::init_local();
    let actor = root_ctx.spawn(DefaultSnapshot { value: "test".to_string() });
    
    let result = actor.ask(GetValue).await.unwrap();
    assert_eq!(result, "test");
}
