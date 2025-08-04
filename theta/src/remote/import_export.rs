use futures::channel::oneshot;
use url::Url;
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    monitor::ReportTx,
    prelude::ActorRef,
    remote::ActorTypeId,
};

pub type ReplyKey = Uuid;

pub trait Importer {
    fn is_imported(&self, actor_id: ActorId) -> bool;

    fn import<A: Actor>(
        &self,
        url: &Url,
    ) -> impl Future<Output = anyhow::Result<ActorRef<A>>> + Send;

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

    fn observe<A: Actor>(
        &self,
        url: &Url,
    ) -> impl Future<Output = anyhow::Result<ReportTx<A>>> + Send;
}

pub trait Exporter {
    fn is_exported(&self, actor_id: ActorId) -> bool;

    // Once exported, it should expect anyone can import or send forward messages to it
    fn export<A: Actor>(&self, actor: ActorRef<A>) -> anyhow::Result<()>;

    fn arrange_reply(&self) -> anyhow::Result<(ReplyKey, oneshot::Receiver<Vec<u8>>)>; // Serialized return value
}
