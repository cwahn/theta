use std::sync::{Arc, Mutex};

use theta_flume::unbounded_with_id;
use uuid::Uuid;

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
    pub async fn spawn<Args>(&self, args: Args) -> ActorRef<Args::Actor>
    where
        Args: ActorArgs,
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
    let id = Uuid::new_v4();
    let (msg_tx, msg_rx) = unbounded_with_id(id);
    let (sig_tx, sig_rx) = unbounded_with_id(id);
    // ! AboutHandle is not unwind safe
    // let (abort_hdl, abort_reg) = AbortHandle::new_pair();

    let actor_hdl = ActorHdl(sig_tx);
    let actor = ActorRef(msg_tx);

    tokio::spawn({
        let actor = actor.downgrade();
        let parent_hdl = parent_hdl.clone();
        let actor_hdl = actor_hdl.clone();

        async move {
            let config = ActorConfig::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, args);
            config.exec().await;
        }
    });

    (actor_hdl, actor)
}
