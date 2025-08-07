use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorConfig},
    actor_instance::ActorConfigImpl,
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
    // pub async fn spawn<B: Actor>(&self, args: B::Args) -> ActorRef<B> {
    pub async fn spawn<C>(&self, args: C) -> ActorRef<C::Actor>
    where
        C: ActorConfig,
    {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }
}

// pub(crate) async fn spawn_impl<C>(parent_hdl: &ActorHdl, args: C::Args) -> (ActorHdl, ActorRef<C>)
pub(crate) async fn spawn_impl<C>(parent_hdl: &ActorHdl, cfg: C) -> (ActorHdl, ActorRef<C::Actor>)
where
    C: ActorConfig,
{
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    let (sig_tx, sig_rx) = mpsc::unbounded_channel();

    let actor_hdl = ActorHdl(sig_tx);
    let actor = ActorRef {
        id: Uuid::new_v4(),
        tx: msg_tx,
    };

    tokio::spawn({
        let actor = actor.downgrade();
        let parent_hdl = parent_hdl.clone();
        let actor_hdl = actor_hdl.clone();

        async move {
            let config = ActorConfigImpl::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, cfg);

            config.exec().await;
        }
    });

    (actor_hdl, actor)
}
