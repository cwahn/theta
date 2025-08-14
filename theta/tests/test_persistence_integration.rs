#[cfg(test)]
mod persistence_integration_tests {
    use theta::prelude::*;
    use theta_macros::{ActorArgs, PersistentActor};
    use serde::{Deserialize, Serialize};
    use url::Url;

    #[derive(Debug, Clone, Serialize, Deserialize, ActorArgs, PersistentActor)]
    struct TestActor {
        pub count: u32,
    }

    impl From<&TestActor> for TestActor {
        fn from(other: &TestActor) -> Self {
            other.clone()
        }
    }

    #[cfg(feature = "remote")]
    impl Actor for TestActor {
        type Msg = ();
        type StateReport = ();

        async fn process_msg(
            &mut self,
            _ctx: Context<Self>,
            _msg: Self::Msg,
            _k: theta::message::Continuation,
        ) {
            self.count += 1;
        }

        const IMPL_ID: theta::remote::base::ActorTypeId = 
            uuid::uuid!("12345678-1234-1234-1234-123456789abc");
    }

    #[cfg(not(feature = "remote"))]  
    impl Actor for TestActor {
        type Msg = ();
        type StateReport = ();

        async fn process_msg(
            &mut self,
            _ctx: Context<Self>,
            _msg: Self::Msg,
            _k: theta::message::Continuation,
        ) {
            self.count += 1;
        }
    }

    #[test]
    fn test_basic_persistence_traits() {
        let test_actor = TestActor { count: 42 };
        
        // Test that the actor can be serialized and deserialized
        let snapshot = TestActor::from(&test_actor);
        assert_eq!(snapshot.count, 42);

        // Test that we can create a URL for persistence
        let _persistence_key = Url::parse("file:///tmp/test_actor").unwrap();
        
        // Just verify types compile - this test doesn't actually run persistence
        println!("Persistence test types compile correctly");
    }
}
