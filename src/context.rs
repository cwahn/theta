use tokio::sync::mpsc;

use crate::{
    actor::Actor,
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, WeakActorHdl, WeakActorRef},
};

#[derive(Debug)]
pub struct Context<'a, A>
where
    A: Actor,
{
    pub this: &'a WeakActorRef<A>,                    // Self reference
    pub(crate) child_hdls: &'a mut Vec<WeakActorHdl>, // children of this actor
    pub(crate) this_hdl: &'a ActorHdl,                // Self supervision reference
}

// Implementations

impl<'a, A> Context<'a, A>
where
    A: Actor,
{
    pub async fn spawn<B: Actor>(&mut self, args: B::Args) -> ActorRef<B> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;

        self.child_hdls.push(actor_hdl.downgrade());

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
    let actor = ActorRef(msg_tx);

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
