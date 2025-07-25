pub mod channel;
pub mod remote_actor;
pub mod serde;

use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

use rustc_hash::FxHashMap;

use crate::base::ImplId;

pub(crate) static REGISTRY: LazyLock<RwLock<FxHashMap<ImplId, Box<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

pub(crate) struct ActorInitFn(pub(crate) fn());
inventory::collect!(ActorInitFn);

pub(crate) struct MsgEntry {
    pub(crate) actor_impl_id: ImplId,
    pub(crate) register_fn: fn(&mut Box<dyn Any + Send + Sync>), // Which will be each actor's deserialize function registry
}
inventory::collect!(MsgEntry);
