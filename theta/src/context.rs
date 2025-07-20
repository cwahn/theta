use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    actor::Actor,
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, WeakActorHdl, WeakActorRef},
};

#[derive(Debug, Clone)]
pub struct Context<A>
where
    A: Actor,
{
    pub this: WeakActorRef<A>,                            // Self reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of this actor
    pub(crate) this_hdl: ActorHdl,                        // Self supervision reference
}

// Implementations

impl<A> Context<A>
where
    A: Actor,
{
    pub async fn spawn<B: Actor>(&self, args: B::Args) -> ActorRef<B> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }
}

pub(crate) async fn spawn_impl<C>(parent_hdl: &ActorHdl, args: C::Args) -> (ActorHdl, ActorRef<C>)
where
    C: Actor,
{
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    let (sig_tx, sig_rx) = mpsc::unbounded_channel();

    let actor_hdl = ActorHdl(sig_tx);
    let actor = ActorRef(Uuid::new_v4(), msg_tx);

    let config = ActorConfig::new(
        actor.downgrade(),
        parent_hdl.clone(),
        actor_hdl.clone(),
        sig_rx,
        msg_rx,
        args,
    );

    tokio::spawn(async move {
        config.exec().await;
    });

    (actor_hdl, actor)
}
