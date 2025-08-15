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

pub(crate) trait DefaultHashCode {
    fn __hash_code(&self) -> u64;
}

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

// Two-level (auto*deref*-based) specialization inside the method
impl<A: Actor> DefaultHashCode for A {
    #[inline]
    fn __hash_code(&self) -> u64 {
        // Wrapper avoids interference with blanket impls on &T (see refs)
        struct Wrap<'a, T>(&'a T);

        trait Dispatch {
            fn f(self) -> u64;
        }

        // Fallback
        impl<'a, T: Actor> Dispatch for &'a Wrap<'a, T> {
            #[inline]
            fn f(self) -> u64 {
                0
            }
        }

        // Preferred when T: Hash — NOTE the two different lifetimes.
        impl<'a, 'b, T> Dispatch for &'b &'a Wrap<'a, T>
        where
            T: Actor + Hash,
        {
            #[inline]
            fn f(self) -> u64 {
                let mut h = FxHasher::default();
                Hash::hash(self.0, &mut h);
                h.finish()
            }
        }

        (&&Wrap(self)).f()
    }
}

impl<T: Actor> From<&T> for Nil {
    fn from(_: &T) -> Self {
        Nil
    }
}

#[cfg(test)]
mod tests {
    use theta_macros::actor;

    use super::*;

    #[test]
    fn test_hash_code() {
        #[derive(Debug, Clone, Hash, Serialize, Deserialize)]
        struct SomeActor {
            id: u64,
        }

        #[actor("d5b79df0-edff-44ef-80e6-7ada5a188f47")]
        impl Actor for SomeActor {
            type StateReport = Self;
        }

        impl From<&SomeActor> for SomeActor {
            fn from(actor: &SomeActor) -> Self {
                actor.clone()
            }
        }

        // Check if more then 99 out of 100 differnt balue's hash_code is non-zero
        let mut non_zero_count = 0;
        let total_count = 100;

        for i in 0..total_count {
            let actor = SomeActor { id: i };
            if actor.__hash_code() != 0 {
                non_zero_count += 1;
            }
        }

        assert_eq!(non_zero_count, 100);
    }
}
