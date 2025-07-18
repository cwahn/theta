use std::ops::Deref;

/// Holds ownership, but cannot be modified outside of this crate.
pub struct Immutable<'a, T>(pub(crate) &'a mut T);

pub(crate) fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Ok(s) = payload.downcast::<String>() {
        *s
    } else {
        "Unknown panic payload".to_string()
    }
}

// Implementations

impl<'a, T> Deref for Immutable<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a, T> AsRef<T> for Immutable<'a, T> {
    fn as_ref(&self) -> &T {
        self.0
    }
}
