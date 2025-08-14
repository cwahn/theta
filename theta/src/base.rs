use std::{borrow::Cow, sync::LazyLock};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::actor::Actor;

pub type Ident = Cow<'static, [u8]>;

// ? Maybe use dyn-clone?
// pub(crate) type AnyActorRef = Arc<dyn AnyActorRef + Send + Sync>; // ActorRef<A>

pub type DefaultRemote = Nil;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

// todo Open control to user
pub(crate) static PROJECT_DIRS: LazyLock<ProjectDirs> = LazyLock::new(|| {
    ProjectDirs::from("com", "example", "theta")
        .expect("Failed to get project directories on this OS")
});

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

impl<T> From<&T> for Nil
where
    T: Actor,
{
    fn from(_: &T) -> Self {
        Nil
    }
}
