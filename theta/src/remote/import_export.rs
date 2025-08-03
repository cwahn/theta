#[cfg(feature = "std")]
use std::borrow::Cow;

#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
use alloc::borrow::Cow;

use futures::channel::oneshot;
use url::Url;
use uuid::Uuid;

use crate::{actor::Actor, prelude::ActorRef};

pub type ReplyKey = Uuid;
pub type Ident = Cow<'static, [u8]>;

// Actor's id will not be valid across the network, since it is just a memeory address.

pub trait Importer {
    // Whild doing deserialization, it should look up the actor in separate task
    fn lookup_local<A: Actor>(&self, ident: Ident) -> anyhow::Result<ActorRef<A>>;

    fn lookup<A: Actor>(
        &self,
        url: &Url,
    ) -> impl Future<Output = anyhow::Result<ActorRef<A>>> + Send;

    // Reply knows exact type, and Return in always serializable.
    // When doing desrialization, it should be done with the exporter.
    fn reply<R: Send + Sync + 'static>(
        &self,
        key: ReplyKey,
        value: R,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;

    // Forward don't know exact type, but only the type of the actor.
    // So it should pass the full actor reference of recepient.
    // When deserializing the message pack, if the continuation is the forward, it should look up the actor,
    // And need to arrange the return to be sent to the actor.
    // So no separate reply but just regular lookup would do the job.
    fn observe<A: Actor>(&self, url: &Url) -> impl Future<Output = anyhow::Result<()>> + Send;
}

pub trait Exporter {
    // While serializing out going message, it should make ad-hoc binding
    fn bind<A: Actor>(&self, ident: Ident, actor: ActorRef<A>) -> anyhow::Result<()>;

    fn free<A: Actor>(&self, ident: Ident) -> anyhow::Result<()>;

    fn arrange_reply<R>(&self) -> anyhow::Result<(ReplyKey, oneshot::Receiver<R>)>
    where
        R: Send + Sync + 'static;
}
