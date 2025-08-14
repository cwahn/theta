use theta::prelude::*;
use theta_macros::{ActorArgs, PersistentActor};
use serde::{Deserialize, Serialize};
use url::Url;
use std::collections::HashMap;

#[derive(Debug, Clone, ActorArgs, PersistentActor)]
struct TestActor {
    pub count: u32,
}

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

fn main() {
    println!("Persistence module test compiled successfully!");
    
    let ctx = RootContext::default();
    let test_actor = TestActor { count: 0 };
    let actor_ref = ctx.spawn(test_actor);
    
    println!("Actor created with ID: {:?}", actor_ref.id());
}
