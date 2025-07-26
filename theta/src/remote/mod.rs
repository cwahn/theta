pub mod channel;
pub mod registry;
pub mod remote_actor;
pub mod serde;

// Re-exports
pub use registry::init_registry;
pub(crate) use registry::{RegisterActorFn, MsgEntry, REGISTRY};
