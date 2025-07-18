use tokio::sync::{Mutex, mpsc};

use crate::{
    actor::Actor,
    actor_ref::{ActorRef, SupervisionRef, WeakActorRef, WeakSupervisionRef},
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

// #[derive(Debug)]
// pub struct WeakContext<'a, A>
// where
//     A: Actor,
// {
//     pub self_ref: WeakActorRef<A>, // Self reference
//     pub(crate) child_refs: &'a mut Vec<WeakSupervisionRef>, // children of this actor
// }

/// Context but additional full access to child refs
#[derive(Debug)]
pub struct SupervisionContext<'a, A: Actor> {
    pub self_ref: ActorRef<A>,                       // Self reference
    pub child_refs: &'a mut Vec<WeakSupervisionRef>, // children of this actor
    pub(crate) self_supervision_ref: SupervisionRef, // Self supervision reference
}

// #[derive(Debug)]
// pub struct WeakSupervisionContext<'a, A: Actor> {
//     pub self_ref: WeakActorRef<A>,                   // Self reference
//     pub child_refs: &'a mut Vec<WeakSupervisionRef>, // children of this actor
// }
//
// Implementations

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

// impl<A> core::clone::Clone for Context<A>
// where
//     A: Actor,
// {
//     fn clone(&self) -> Self {
//         match self {
//             Context {
//                 self_ref: actor_ref,
//                 child_refs: children,
//             } => Context {
//                 self_ref: actor_ref.clone(),
//                 child_refs: children.clone(),
//             },
//         }
//     }
// }

// impl<A> WeakContext<A>
// where
//     A: Actor,
// {
//     pub(crate) fn upgrade(self) -> Option<Context<A>> {
//         let Some(self_ref) = self.self_ref.upgrade() else {
//             return None;
//         };

//         Some(Context {
//             self_ref,
//             child_refs: self.child_refs,
//         })
//     }

//     pub(crate) fn upgraded(&self) -> Option<Context<A>> {
//         let Some(self_ref) = self.self_ref.upgrade() else {
//             return None;
//         };

//         Some(Context {
//             self_ref,
//             child_refs: self.child_refs.clone(),
//         })
//     }
// }

// impl<A> Clone for WeakContext<A>
// where
//     A: Actor,
// {
//     fn clone(&self) -> Self {
//         WeakContext {
//             self_ref: self.self_ref.clone(),
//             child_refs: self.child_refs.clone(),
//         }
//     }
// }

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

    // Who will keep the SupervisionRef not to dropped => Self,
    // Who will keep the ActorRef not to dropped => Self

    todo!();

    // actor_ref
}

// impl<A> WeakSupervisionContext<A>
// where
//     A: Actor,
// {
//     pub(crate) fn upgrade(self) -> Option<SupervisionContext<A>> {
//         let Some(self_ref) = self.self_ref.upgrade() else {
//             return None;
//         };

//         let Some(parent_ref) = self.parent_ref.upgrade() else {
//             return None;
//         };

//         Some(SupervisionContext {
//             self_ref,
//             parent_ref,
//             child_refs: self.child_refs,
//         })
//     }

//     pub(crate) fn upgraded(&self) -> Option<SupervisionContext<A>> {
//         let Some(self_ref) = self.self_ref.upgrade() else {
//             return None;
//         };

//         let Some(parent_ref) = self.parent_ref.upgrade() else {
//             return None;
//         };

//         Some(SupervisionContext {
//             self_ref,
//             parent_ref,
//             child_refs: self.child_refs.clone(),
//         })
//     }

//     pub(crate) fn regular(self) -> WeakContext<A> {
//         WeakContext {
//             self_ref: self.self_ref,
//             child_refs: self.child_refs,
//         }
//     }
// }
