pub mod codec;
pub mod frame_codec;
pub mod peer;
pub mod registry;

// Re-exports

pub use registry::{ACTOR_REGISTRY, ActorEntry, ActorRegistry, RegisterActorFn};
