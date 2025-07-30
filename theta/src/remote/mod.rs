pub mod codec;
pub mod peer;
pub mod registry;
pub mod serde;

// Re-exports
// pub use registry::init_registry;
pub use registry::{
    ACTOR_REGISTRY, ActorEntry, ActorRegistry, MsgEntry, MsgRegistry, RegisterActorFn,
    RegisterBehaviorFn,
};
