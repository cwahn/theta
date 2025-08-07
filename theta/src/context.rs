use std::sync::{Arc, Mutex};

use futures::stream::{AbortHandle, Abortable};
use tokio::sync::mpsc;

use crate::{
    actor::{Actor, ActorArgs},
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, WeakActorHdl, WeakActorRef},
};

#[derive(Debug, Clone)]
pub struct Context<A>
where
    A: Actor,
{
    pub this: WeakActorRef<A>,                            // Self reference
    pub(crate) this_hdl: ActorHdl,                        // Self supervision reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of this actor
}

// Implementations

impl<A> Context<A>
where
    A: Actor,
{
    // pub async fn spawn<B: Actor>(&self, args: B::Args) -> ActorRef<B> {
    pub async fn spawn<C>(&self, args: C) -> ActorRef<C::Actor>
    where
        C: ActorArgs,
    {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }
}

pub(crate) async fn spawn_impl<Args>(
    parent_hdl: &ActorHdl,
    args: Args,
) -> (ActorHdl, ActorRef<Args::Actor>)
where
    Args: ActorArgs,
{
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    let (sig_tx, sig_rx) = mpsc::unbounded_channel();
    let (abort_hdl, abort_reg) = AbortHandle::new_pair();

    let actor_hdl = ActorHdl(sig_tx, abort_hdl);
    let actor = ActorRef::new(msg_tx);

    tokio::spawn(Abortable::new(
        {
            let actor = actor.downgrade();
            let parent_hdl = parent_hdl.clone();
            let actor_hdl = actor_hdl.clone();

            async move {
                let config = ActorConfig::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, args);

                config.exec().await;
            }
        },
        abort_reg,
    ));

    (actor_hdl, actor)
}
