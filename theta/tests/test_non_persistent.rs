use theta::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct NonPersistentActor {
    pub value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

// Test without snapshot attribute
#[actor("847d1a75-bf42-4690-b947-c3f206fda4cf")]
impl Actor for NonPersistentActor {
    const _: () = {
        async |_: GetValue| -> i32 { self.value };
    };
}

#[tokio::test]
async fn test_non_persistent_actor() {
    let root_ctx = RootContext::init_local();
    let actor = root_ctx.spawn(NonPersistentActor { value: 42 });
    
    let result = actor.ask(GetValue).await.unwrap();
    assert_eq!(result, 42);
}
