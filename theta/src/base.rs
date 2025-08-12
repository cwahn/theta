use std::{
    any::Any,
    borrow::Cow,
    sync::{LazyLock, RwLock},
};

use directories::ProjectDirs;
use rustc_hash::FxHashMap;

#[cfg(feature = "remote")]
use crate::remote::base::ExportTaskFn;

pub type Ident = Cow<'static, [u8]>;

pub(crate) type Bindings = RwLock<FxHashMap<Ident, Binding>>;

pub(crate) struct Binding {
    pub(crate) actor: AnyActorRef,
    #[cfg(feature = "remote")]
    pub(crate) export_task_fn: ExportTaskFn,
}

pub(crate) type AnyActorRef = Box<dyn Any + Send + Sync>; // ActorRef<A>

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
