use theta::prelude::*;
use theta::persistence::persistent_actor::PersistentActor;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct TestActor {
    pub value: i32,
}

impl From<&TestActor> for TestActor {
    fn from(actor: &TestActor) -> Self {
        Self { value: actor.value }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValue;

// Test with explicit snapshot type
#[actor("847d1a75-bf42-4690-b947-c3f206fda4cf", snapshot = TestActor)]
impl Actor for TestActor {
    const _: () = {
        async |_: GetValue| -> i32 { self.value };
    };
}

#[tokio::test]
async fn test_persistent_actor_trait_implementation() {
    // This test verifies that the PersistentActor trait is implemented
    fn assert_persistent_actor<T: PersistentActor>() {}
    
    assert_persistent_actor::<TestActor>();
}
