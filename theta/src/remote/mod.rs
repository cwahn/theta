pub mod codec;
pub mod import_export;
pub mod peer;
pub mod registry;
pub mod serde;

// Re-exports

pub use serde::{
    ACTOR_REGISTRY, ActorEntry, ActorRegistry, BehaviorEntry, BehaviorRegistry, RegisterActorFn,
    RegisterBehaviorFn,
};

use uuid::Uuid;

pub type TypeId = Uuid;
pub type ActorTypeId = TypeId;
pub type MsgTypeId = TypeId;

// todo #[tid = "Uuid"] macro
pub trait Remote {
    const TYPE_ID: TypeId;
}
