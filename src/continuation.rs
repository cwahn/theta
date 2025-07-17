use std::any::Any;

use tokio::sync::oneshot;

use crate::{actor_ref::DynMainRef, error::MsgError};

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
///
// pub type Continuation = oneshot::Sender<Box<dyn Any + Send>>;

pub enum Continuation {
    Reply(oneshot::Sender<Box<dyn Any + Send>>),
    Actor(DynMainRef),
}

impl Continuation {
    pub fn send(self, value: Box<dyn Any + Send>) -> Result<(), MsgError<Box<dyn Any + Send>>> {
        match self {
            Continuation::Reply(sender) => {
                if let Err(x) = sender.send(value) {
                    return Err(MsgError::LocalClosed(x.into()));
                }
                Ok(())
            }
            Continuation::Actor(k_ref) => k_ref.send(value),
        }
    }
}
