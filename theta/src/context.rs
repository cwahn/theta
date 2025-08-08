use std::sync::{Arc, Mutex};

use theta_flume::unbounded_with_id;
use uuid::Uuid;

#[cfg(feature = "tracing")]
use tracing::{debug, error};

use crate::{
    actor::{Actor, ActorArgs},
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, WeakActorHdl, WeakActorRef},
    message::RawSignal,
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

#[derive(Debug, Clone)]
pub struct RootContext {
    pub(crate) this_hdl: ActorHdl,                        // Self reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of the global context
}

// Implementations

impl<A: Actor> Context<A> {
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }
}

impl RootContext {
    pub fn new() -> Self {
        let (sig_tx, sig_rx) = unbounded_with_id(Uuid::new_v4());
        let this_hdl = ActorHdl(sig_tx);
        let child_hdls = Arc::new(Mutex::new(Vec::<WeakActorHdl>::new()));

        tokio::spawn({
            let child_hdls = child_hdls.clone();

            async move {
                while let Some(sig) = sig_rx.recv().await {
                    match sig {
                        RawSignal::Escalation(e, escalation) => {
                            #[cfg(feature = "tracing")]
                            error!("Escalation received: {escalation:?} for actor: {e:?}");

                            e.raw_send(RawSignal::Terminate(None)).unwrap();
                        }
                        RawSignal::ChildDropped => {
                            #[cfg(feature = "tracing")]
                            debug!("A top-level actor has been dropped.");

                            let mut child_hdls = child_hdls.lock().unwrap();
                            child_hdls.retain(|hdl| hdl.0.strong_count() > 0);
                        }
                        _ => unreachable!(),
                    }
                }
            }
        });

        Self {
            this_hdl,
            child_hdls,
        }
    }

    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }
}

pub(crate) fn spawn_impl<Args>(
    parent_hdl: &ActorHdl,
    args: Args,
) -> (ActorHdl, ActorRef<Args::Actor>)
where
    Args: ActorArgs,
{
    let id = Uuid::new_v4();
    let (msg_tx, msg_rx) = unbounded_with_id(id);
    let (sig_tx, sig_rx) = unbounded_with_id(id);

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
