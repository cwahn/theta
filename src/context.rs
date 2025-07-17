use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use crate::{
    actor::Actor,
    actor_ref::{ActorRef, MainRef, SupervisionRef, WeakActorRef, WeakSupervisionRef},
};

#[derive(Debug)]
pub struct Context<A>
where
    A: Actor,
{
    pub self_ref: ActorRef<A>, // Self reference
    pub(crate) child_refs: Arc<Mutex<Vec<WeakSupervisionRef>>>, // children of this actor
}

#[derive(Debug)]
pub struct WeakContext<A>
where
    A: Actor,
{
    pub self_ref: WeakActorRef<A>, // Self reference
    pub(crate) child_refs: Arc<Mutex<Vec<WeakSupervisionRef>>>, // children of this actor
}

// ? Should regular context should have access to children?

#[derive(Debug, Clone)]
pub struct SupervisionContext<A: Actor> {
    pub self_ref: ActorRef<A>,             // Self reference
    pub(crate) parent_ref: SupervisionRef, // Parent reference
    pub(crate) child_refs: Arc<Mutex<Vec<WeakSupervisionRef>>>, // children of this actor
}

#[derive(Debug, Clone)]
pub struct WeakSupervisionContext<A: Actor> {
    pub self_ref: WeakActorRef<A>,             // Self reference
    pub(crate) parent_ref: WeakSupervisionRef, // Parent reference
    pub(crate) child_refs: Arc<Mutex<Vec<WeakSupervisionRef>>>, // children of this actor
}

// Implementations

impl<A> Context<A>
where
    A: Actor,
{
    pub fn new(self_ref: ActorRef<A>) -> Self {
        Self {
            self_ref,
            child_refs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn spawn<B: Actor>(&self, args: B::Args) -> ActorRef<B> {
        spawn(&self.self_ref, args).await
    }
}

impl<A> core::clone::Clone for Context<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        match self {
            Context {
                self_ref: actor_ref,
                child_refs: children,
            } => Context {
                self_ref: actor_ref.clone(),
                child_refs: children.clone(),
            },
        }
    }
}

impl<A> WeakContext<A>
where
    A: Actor,
{
    pub(crate) fn upgrade(self) -> Option<Context<A>> {
        let Some(self_ref) = self.self_ref.upgrade() else {
            return None;
        };

        Some(Context {
            self_ref,
            child_refs: self.child_refs,
        })
    }

    pub(crate) fn upgraded(&self) -> Option<Context<A>> {
        let Some(self_ref) = self.self_ref.upgrade() else {
            return None;
        };

        Some(Context {
            self_ref,
            child_refs: self.child_refs.clone(),
        })
    }
}

impl<A> Clone for WeakContext<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakContext {
            self_ref: self.self_ref.clone(),
            child_refs: self.child_refs.clone(),
        }
    }
}

impl<A> SupervisionContext<A>
where
    A: Actor,
{
    // pub fn self_ref(&self) -> &ActorRef<A> {
    //     &self.self_ref
    // }

    pub async fn spawn<B: Actor>(&self, args: B::Args) -> ActorRef<B> {
        spawn(&self.self_ref, args).await
    }
}

async fn spawn<P, C>(parent_ref: &ActorRef<P>, args: C::Args) -> ActorRef<C>
where
    P: Actor,
    C: Actor,
{
    let (message_tx, message_rx) = mpsc::unbounded_channel();
    let (signal_tx, signal_rx) = mpsc::unbounded_channel();

    let new_actor_ref = ActorRef {
        main_tx: MainRef(message_tx),
        signal_tx: SupervisionRef(signal_tx),
    };

    let parent_ref = parent_ref.signal_tx.clone();

    tokio::spawn(run(
        args,
        new_actor_ref.downgrade(),
        parent_ref,
        message_rx,
        signal_rx,
    ));

    new_actor_ref
}

impl<A> WeakSupervisionContext<A>
where
    A: Actor,
{
    pub(crate) fn upgrade(self) -> Option<SupervisionContext<A>> {
        let Some(self_ref) = self.self_ref.upgrade() else {
            return None;
        };

        let Some(parent_ref) = self.parent_ref.upgrade() else {
            return None;
        };

        Some(SupervisionContext {
            self_ref,
            parent_ref,
            child_refs: self.child_refs,
        })
    }

    pub(crate) fn upgraded(&self) -> Option<SupervisionContext<A>> {
        let Some(self_ref) = self.self_ref.upgrade() else {
            return None;
        };

        let Some(parent_ref) = self.parent_ref.upgrade() else {
            return None;
        };

        Some(SupervisionContext {
            self_ref,
            parent_ref,
            child_refs: self.child_refs.clone(),
        })
    }

    pub(crate) fn regular(self) -> WeakContext<A> {
        WeakContext {
            self_ref: self.self_ref,
            child_refs: self.child_refs,
        }
    }
}
