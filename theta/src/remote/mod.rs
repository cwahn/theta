pub mod channel;
pub mod remote_actor;
pub mod serde;

use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

use rustc_hash::FxHashMap;
// use serde::{Deserialize, Serialize};
use serde_flexitos::Registry;
use theta_macros::ActorConfig;
use uuid::uuid;

use crate::{
    actor::Actor,
    base::ImplId,
    message::{Behavior, Continuation, Message},
    prelude::GlobalContext,
    remote::serde::DeserializeFnRegistry,
};

pub(crate) static REGISTRY: LazyLock<RwLock<FxHashMap<ImplId, Box<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

pub(crate) struct ActorInitFn(pub(crate) fn());
inventory::collect!(ActorInitFn);

pub(crate) struct MsgEntry {
    pub(crate) actor_impl_id: ImplId,
    pub(crate) register_fn: fn(&mut Box<dyn Any + Send + Sync>), // Which will be each actor's deserialize function registry
}
inventory::collect!(MsgEntry);
