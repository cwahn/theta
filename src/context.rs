use tokio::sync::mpsc;

use crate::{
    actor::Actor,
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef},
};

#[derive(Debug)]
pub struct Context<'a, A>
where
    A: Actor,
{
    pub this: &'a ActorRef<A>,                    // Self reference
    pub(crate) child_hdls: &'a mut Vec<ActorHdl>, // children of this actor
    pub(crate) this_hdl: &'a ActorHdl,            // Self supervision reference
}

/// Context but additional full access to child refs
#[derive(Debug)]
pub struct SuperContext<'a, A: Actor> {
    pub this: &'a ActorRef<A>,             // Self reference
    pub child_hdls: &'a mut Vec<ActorHdl>, // children of this actor
    pub(crate) this_hdl: &'a ActorHdl,     // Self supervision reference
}

impl<'a, A> Context<'a, A>
where
    A: Actor,
{
    pub async fn spawn<B: Actor>(&mut self, args: B::Args) -> ActorRef<B> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;

        self.child_hdls.push(actor_hdl);

        actor
    }
}

impl<'a, A> SuperContext<'a, A>
where
    A: Actor,
{
    pub async fn spawn<B: Actor>(&mut self, args: B::Args) -> ActorRef<B> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;

        self.child_hdls.push(actor_hdl);

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
        actor.clone(),
        actor_hdl.clone(),
        parent_hdl.clone(),
        args,
        sig_rx,
        msg_rx,
    );

    tokio::spawn(async move {
        config.exec().await;
    });

    // Who will keep the SupervisionRef not to dropped => Self,
    // Who will keep the ActorRef not to dropped => Self

    (actor_hdl, actor)
}
