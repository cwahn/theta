use futures::{channel::oneshot, future::BoxFuture};
use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;
use uuid::Uuid;

use crate::{actor::ActorId, remote::ActorTypeId};

pub type ReplyKey = Uuid;

// It's hard to decide if I would make it dyn compatible or not.
task_local! {
    pub static IMPORTER: dyn Importer;
    pub static EXPORTER: dyn Exporter;
}
// While deserializing it might need to import from another host or protocol.

// todo Use no_std compatible AsyncRead/AsyncWrite
// So that actor could be reached from serial communication.

pub trait Importer {
    fn is_imported(&self, actor_id: ActorId) -> bool;

    // Need to deserialize it with the importer.
    // Should write serialized Message<A> to the stream
    fn import(
        &self,
        url: &Url,
        actor_type_id: ActorTypeId,
    ) -> BoxFuture<anyhow::Result<Box<dyn AsyncWrite + Send + Sync>>>;

    fn reply(
        &self,
        key: ReplyKey,
        return_bytes: Vec<u8>, // Serialized return value
    ) -> anyhow::Result<()>;

    fn forward(
        &self,
        url: &Url,
        actor_type_id: ActorTypeId,
        msg_bytes: Vec<u8>, // Serialized BoxedMsg<A>
    ) -> anyhow::Result<()>;

    /// Get the current state of the actor
    fn get_state(
        &self,
        actor_type_id: ActorTypeId,
        url: &Url,
    ) -> BoxFuture<anyhow::Result<oneshot::Receiver<Vec<u8>>>>; // Serialized A::StateReport

    /// Listen for the Report
    fn observe(
        &self,
        actor_type_id: ActorTypeId,
        actor_id: ActorId,
    ) -> BoxFuture<anyhow::Result<Box<dyn AsyncRead + Send + Sync>>>; // The byte stream emits ActorReport<A>
}

pub trait Exporter {
    // fn is_exported(&self, actor_id: ActorId) -> bool;

    // // Once exported, it should expect anyone can import or send forward messages to it
    // // Will get the new stream on each new import form
    // fn export(
    //     &self,
    //     actor_type_id: ActorTypeId,
    //     ident: Ident,
    // ) -> anyhow::Result<UnboundedReceiver<Box<dyn AsyncRead + Send + Sync>>>;

    // If it is bound, it should be considered to be exported ?
    // So whenever serialize ActorRef<A> make unanimous binding with the actor_id.

    // ? Binding to where?
    // It should be current protocol.

    // The problem is when should I kill the exported or bound actor?
    // Should I have concept of exporting through which protocol?

    // Type will be know for both side
    fn arrange_reply(&self) -> anyhow::Result<(ReplyKey, oneshot::Receiver<Vec<u8>>)>; // Serialized return value
}

pub trait Sometrait {
    fn some_method(&self) -> BoxFuture<()>;
}
