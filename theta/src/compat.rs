//! Platform compatibility layer for native/WASM targets.
//!
//! Abstracts `tokio::spawn`, `tokio::time`, `tokio::select!`, `DashMap`, and `task_local!`
//! behind a unified API that works on both native (multi-threaded tokio) and
//! browser WASM (single-threaded `wasm_bindgen_futures::spawn_local`).
use std::future::Future;

pub use map::*;

/// On native: delegates to `tokio::task_local!`.
/// On WASM: uses `thread_local! + RefCell` with scope-based restore.
#[cfg(not(wasm_browser))]
macro_rules! compat_task_local {
    ($(#[$attr:meta])* $vis:vis static $name:ident : $ty:ty;) => {
        mod __compat_tl_inner { use super::*; tokio::task_local! { pub (super) static INNER : $ty; }
        } $(#[$attr])* $vis struct $name; impl $name { pub fn get(&self) -> $ty {
        __compat_tl_inner::INNER.get() } pub fn try_get(&self) -> Option <$ty > {
        __compat_tl_inner::INNER.try_get().ok() } pub fn with < R > (&self, f : impl FnOnce(&$ty) ->
        R) -> R { __compat_tl_inner::INNER.with(f) } pub async fn scope < F : std::future::Future >
        (&self, value : $ty, fut : F) -> F::Output { __compat_tl_inner::INNER.scope(value, fut).
        await } pub fn sync_scope < R > (&self, value : $ty, f : impl FnOnce() -> R) -> R {
        __compat_tl_inner::INNER.sync_scope(value, f) } #[doc =
        " Run closure with task-local temporarily cleared."] #[doc =
        " On native this is a no-op — `tokio task_local` is already per-task."] #[allow(dead_code)]
        pub fn clear_scope < R > (&self, f : impl FnOnce() -> R) -> R { f() } }
    };
}

#[cfg(wasm_browser)]
macro_rules! compat_task_local {
    ($(#[$attr:meta])* $vis:vis static $name:ident : $ty:ty;) => {
        $(#[$attr])* $vis struct $name; impl $name { thread_local! { static INNER :
        std::cell::RefCell < Option <$ty >> = const { std::cell::RefCell::new(None) }; } pub fn
        get(&self) -> $ty { Self::INNER.with(|cell| { cell.borrow().clone()
        .expect(concat!("task-local `", stringify!($name), "` not set; are you inside a scope?")) })
        } pub fn try_get(&self) -> Option <$ty > { Self::INNER.with(|cell| cell.borrow().clone()) }
        pub fn with < R > (&self, f : impl FnOnce(&$ty) -> R) -> R { Self::INNER.with(|cell| { let
        borrow = cell.borrow(); let val = borrow.as_ref().expect(concat!("task-local `",
        stringify!($name), "` not set; are you inside a scope?")); f(val) }) } pub async fn scope <
        F : std::future::Future > (&self, value : $ty, fut : F) -> F::Output { let prev =
        Self::INNER.with(|cell| cell.borrow_mut().replace(value)); let result = fut. await;
        Self::INNER.with(|cell| * cell.borrow_mut() = prev); result } pub fn sync_scope < R >
        (&self, value : $ty, f : impl FnOnce() -> R) -> R { let prev = Self::INNER.with(|cell| cell
        .borrow_mut().replace(value)); let result = f(); Self::INNER.with(|cell| * cell.borrow_mut()
        = prev); result } #[doc = " Run closure with task-local temporarily cleared."] #[doc =
        " Prevents PEER leak across cooperative WASM tasks."] pub fn clear_scope < R > (&self, f :
        impl FnOnce() -> R) -> R { let prev = Self::INNER.with(|cell| cell.borrow_mut().take()); let
        result = f(); Self::INNER.with(|cell| * cell.borrow_mut() = prev); result } }
    };
}

/// A timeout error, analogous to `tokio::time::error::Elapsed`.
#[derive(Debug, Clone)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}

#[cfg(not(wasm_browser))]
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future);
}

#[cfg(wasm_browser)]
pub(crate) fn spawn<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(not(wasm_browser))]
pub async fn timeout<F: Future>(
    duration: std::time::Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| Elapsed)
}

#[cfg(wasm_browser)]
pub(crate) async fn timeout<F: Future>(
    duration: std::time::Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    n0_future::time::timeout(duration, future)
        .await
        .map_err(|_| Elapsed)
}

#[cfg(not(wasm_browser))]
mod map {
    use std::hash::Hash;

    /// Thin wrapper around `DashMap` exporting a consistent API.
    pub struct ConcurrentMap<K, V>(dashmap::DashMap<K, V>);

    impl<K: Eq + Hash + std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for ConcurrentMap<K, V> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_tuple("ConcurrentMap").field(&self.0).finish()
        }
    }

    impl<K: Eq + Hash, V> Default for ConcurrentMap<K, V> {
        fn default() -> Self {
            Self(dashmap::DashMap::default())
        }
    }

    impl<K: Eq + Hash, V> ConcurrentMap<K, V> {
        pub(crate) fn with_capacity(capacity: usize) -> Self {
            Self(dashmap::DashMap::with_capacity(capacity))
        }
    }

    pub struct OccupiedEntry<'a, K, V>(dashmap::mapref::entry::OccupiedEntry<'a, K, V>);

    pub struct VacantEntry<'a, K, V>(dashmap::mapref::entry::VacantEntry<'a, K, V>);

    pub enum Entry<'a, K, V> {
        Occupied(OccupiedEntry<'a, K, V>),
        Vacant(VacantEntry<'a, K, V>),
    }

    impl<K: Eq + Hash + Clone, V> OccupiedEntry<'_, K, V> {
        pub(crate) fn get(&self) -> &V {
            self.0.get()
        }

        pub(crate) fn insert(&mut self, value: V) -> V {
            self.0.insert(value)
        }

        pub(crate) fn remove(self) -> V {
            self.0.remove()
        }
    }

    impl<K: Eq + Hash + Clone, V> VacantEntry<'_, K, V> {
        pub(crate) fn insert(self, value: V) {
            self.0.insert(value);
        }
    }

    impl<K: Eq + Hash + Clone, V> ConcurrentMap<K, V> {
        pub(crate) fn entry(&self, key: K) -> Entry<'_, K, V> {
            match self.0.entry(key) {
                dashmap::Entry::Occupied(o) => Entry::Occupied(OccupiedEntry(o)),
                dashmap::Entry::Vacant(v) => Entry::Vacant(VacantEntry(v)),
            }
        }
    }

    impl<K: Eq + Hash + Clone, V: Clone> ConcurrentMap<K, V> {
        pub(crate) fn get<Q>(&self, key: &Q) -> Option<V>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            self.0.get(key).map(|r| r.value().clone())
        }
    }

    impl<K: Eq + Hash + Clone, V> ConcurrentMap<K, V> {
        /// Borrow the value behind the `DashMap` read-guard and apply `f`,
        /// avoiding a full value clone.
        pub(crate) fn with<Q, F, R>(&self, key: &Q, f: F) -> Option<R>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
            F: FnOnce(&V) -> R,
        {
            self.0.get(key).map(|r| f(r.value()))
        }

        pub(crate) fn insert(&self, key: K, value: V) -> Option<V> {
            self.0.insert(key, value)
        }

        pub(crate) fn remove<Q>(&self, key: &Q) -> Option<(K, V)>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            self.0.remove(key)
        }
    }
}
#[cfg(wasm_browser)]
mod map {
    use std::{collections::HashMap, hash::Hash, sync::Mutex};

    /// A thin wrapper around `Mutex<HashMap>` that mirrors DashMap's API surface
    /// used in theta. Single-threaded WASM makes the Mutex trivially cheap.
    #[derive(Debug)]
    pub struct ConcurrentMap<K, V>(Mutex<HashMap<K, V>>);

    impl<K, V> Default for ConcurrentMap<K, V> {
        fn default() -> Self {
            Self(Mutex::new(HashMap::new()))
        }
    }

    impl<K, V> ConcurrentMap<K, V> {
        pub(crate) fn with_capacity(capacity: usize) -> Self {
            Self(Mutex::new(HashMap::with_capacity(capacity)))
        }
    }

    pub struct OccupiedEntry<'a, K, V> {
        guard: std::sync::MutexGuard<'a, HashMap<K, V>>,
        key: K,
    }

    pub struct VacantEntry<'a, K, V> {
        guard: std::sync::MutexGuard<'a, HashMap<K, V>>,
        key: K,
        _phantom: std::marker::PhantomData<V>,
    }

    pub enum Entry<'a, K, V> {
        Occupied(OccupiedEntry<'a, K, V>),
        Vacant(VacantEntry<'a, K, V>),
    }

    impl<K: Eq + Hash + Clone, V> OccupiedEntry<'_, K, V> {
        pub(crate) fn get(&self) -> &V {
            self.guard.get(&self.key).unwrap()
        }

        pub(crate) fn insert(&mut self, value: V) -> V {
            self.guard.insert(self.key.clone(), value).unwrap()
        }

        pub(crate) fn remove(mut self) -> V {
            self.guard.remove(&self.key).unwrap()
        }
    }

    impl<K: Eq + Hash + Clone, V> VacantEntry<'_, K, V> {
        pub(crate) fn insert(mut self, value: V) {
            self.guard.insert(self.key.clone(), value);
        }
    }

    impl<K: Eq + Hash + Clone, V> ConcurrentMap<K, V> {
        pub(crate) fn entry(&self, key: K) -> Entry<'_, K, V> {
            let guard = self.0.lock().unwrap();

            if guard.contains_key(&key) {
                Entry::Occupied(OccupiedEntry { guard, key })
            } else {
                Entry::Vacant(VacantEntry {
                    guard,
                    key,
                    _phantom: std::marker::PhantomData,
                })
            }
        }
    }

    impl<K: Eq + Hash + Clone, V: Clone> ConcurrentMap<K, V> {
        pub(crate) fn get<Q>(&self, key: &Q) -> Option<V>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            self.0.lock().unwrap().get(key).cloned()
        }
    }

    impl<K: Eq + Hash + Clone, V> ConcurrentMap<K, V> {
        pub(crate) fn with<Q, F, R>(&self, key: &Q, f: F) -> Option<R>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
            F: FnOnce(&V) -> R,
        {
            let guard = self.0.lock().unwrap();

            guard.get(key).map(f)
        }

        pub(crate) fn insert(&self, key: K, value: V) -> Option<V> {
            self.0.lock().unwrap().insert(key, value)
        }

        pub(crate) fn remove<Q>(&self, key: &Q) -> Option<(K, V)>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            let mut guard = self.0.lock().unwrap();

            guard.remove_entry(key)
        }
    }
}
