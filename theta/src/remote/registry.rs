use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

use rustc_hash::FxHashMap;

use crate::base::ImplId;

pub fn init_registry() {
    for init_fn in inventory::iter::<RegisterActorFn> {
        (init_fn.0)();
    }
}

pub(crate) static REGISTRY: LazyLock<RwLock<FxHashMap<ImplId, Box<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

pub(crate) struct RegisterActorFn(pub(crate) fn());

inventory::collect!(RegisterActorFn);

pub(crate) struct RegisterMsg {
    pub(crate) actor_impl_id: ImplId,
    pub(crate) register_fn: fn(&mut Box<dyn Any + Send + Sync>), // Which will be each actor's deserialize function registry
}

inventory::collect!(RegisterMsg);
