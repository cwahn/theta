pub mod channel;
pub mod registry;
pub mod serde;
// todo pub mod peer;

// Re-exports
pub use registry::init_registry;
pub(crate) use registry::{RegisterBehavior, REGISTRY, RegisterActorFn};
