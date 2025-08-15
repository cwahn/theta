use std::{
    borrow::Cow,
    hash::{Hash, Hasher},
    sync::LazyLock,
};

use directories::ProjectDirs;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::actor::Actor;

// ? Maybe use dyn-clone?
// pub(crate) type AnyActorRef = Arc<dyn AnyActorRef + Send + Sync>; // ActorRef<A>

// todo Open control to user
pub(crate) static PROJECT_DIRS: LazyLock<ProjectDirs> = LazyLock::new(|| {
    ProjectDirs::from("com", "example", "theta")
        .expect("Failed to get project directories on this OS")
});

pub type Ident = Cow<'static, [u8]>;

// pub(crate) trait DefaultHashCode {
//     fn __hash_code(&self) -> u64;
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

pub(crate) fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Ok(s) = payload.downcast::<String>() {
        *s
    } else {
        "Unknown panic payload".to_string()
    }
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::trace!($($arg)*);}
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::debug!($($arg)*);}
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::info!($($arg)*);}
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::warn!($($arg)*);}
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::error!($($arg)*);}
    };
}

// Implementations

// 

impl<T: Actor> From<&T> for Nil {
    fn from(_: &T) -> Self {
        Nil
    }
}
