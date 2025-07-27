pub mod channel;
pub mod peer;
pub mod registry;
pub mod serde;

// Re-exports
pub use registry::init_registry;
pub(crate) use registry::{RegisterMsg, REGISTRY, RegisterActorFn};
