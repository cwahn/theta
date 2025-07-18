use tokio::sync::mpsc;

use crate::{
    actor::Actor,
    actor_instance::ActorConfig,
    actor_ref::{ActorRef, SupervisionRef, WeakSupervisionRef},
};

#[derive(Debug)]
pub struct Context<'a, A>
where
    A: Actor,
{
    pub self_ref: ActorRef<A>,                              // Self reference
    pub(crate) child_refs: &'a mut Vec<WeakSupervisionRef>, // children of this actor
    pub(crate) self_supervision_ref: SupervisionRef,        // Self supervision reference
}

/// Context but additional full access to child refs
#[derive(Debug)]
pub struct SupervisionContext<'a, A: Actor> {
    pub self_ref: ActorRef<A>,                       // Self reference
    pub child_refs: &'a mut Vec<WeakSupervisionRef>, // children of this actor
    pub(crate) self_supervision_ref: SupervisionRef, // Self supervision reference
}

impl<'a, A> Context<'a, A>
where
    A: Actor,
{
    pub async fn spawn<B: Actor>(&mut self, args: B::Args) -> ActorRef<B> {
        let (supervision_ref, actor_ref) = spawn(&self.self_supervision_ref, args).await;

        self.child_refs.push(supervision_ref.downgrade());

        actor_ref
    }
}

impl<'a, A> SupervisionContext<'a, A>
where
    A: Actor,
{
    pub async fn spawn<B: Actor>(&mut self, args: B::Args) -> ActorRef<B> {
        let (supervision_ref, actor_ref) = spawn(&self.self_supervision_ref, args).await;

        self.child_refs.push(supervision_ref.downgrade());

        actor_ref
    }
}

async fn spawn<C>(parent_ref: &SupervisionRef, args: C::Args) -> (SupervisionRef, ActorRef<C>)
where
    C: Actor,
{
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    let (sig_tx, sig_rx) = mpsc::unbounded_channel();

    let supervision_ref = SupervisionRef(sig_tx);
    let actor_ref = ActorRef(msg_tx);

    let config = ActorConfig::new(
        actor_ref.downgrade(),
        supervision_ref.clone(),
        parent_ref.clone(),
        args,
        sig_rx,
        msg_rx,
    );

    tokio::spawn(async move {
        config.exec().await;
    });

    // Who will keep the SupervisionRef not to dropped => Self,
    // Who will keep the ActorRef not to dropped => Self

    (supervision_ref, actor_ref)
}
