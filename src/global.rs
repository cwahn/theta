use std::collections::HashMap;

use crate::{
    actor::Actor,
    actor_ref::{ActorRef, WeakSignalHdl},
};

pub struct GlobalContext {
    child_bindings: HashMap<String, WeakSignalHdl>,
}

impl GlobalContext {
    pub async fn spawn<A: Actor>(&mut self, args: A::Args) -> ActorRef<A> {
        todo!()
    }

    pub async fn bind<A: Actor>(&mut self, name: &str, actor: &ActorRef<A>) {
        todo!()
    }

    pub async fn lookup<A: Actor>(&self, name: &str) -> Option<ActorRef<A>> {
        todo!()
    }

    pub fn free<A: Actor>(&mut self, name: &str) -> Option<WeakSignalHdl> {
        todo!()
    }
}

// let actor = theta::spawn::<SomeActor>(args).await;

// theta::bind("some_actor", actor.clone()).await;

// theta::unbind::<SomeActor>("some_actor").await;

// if let Some(actor_ref) = theta::lookup::<SomeActor>("some_actor").await {
//     // Use actor_ref
// }
