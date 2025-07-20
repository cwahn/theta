use std::{
    any::Any,
    borrow::Cow,
    collections::HashMap,
    sync::{LazyLock, Mutex},
};
use tokio::sync::mpsc;

use crate::{
    actor::Actor,
    actor_ref::{ActorHdl, ActorRef},
    context::spawn_impl,
};

/// Global context for managing root-level actors and actor bindings.
///
/// This provides a singleton interface for:
/// - Spawning root actors without a parent context
/// - Binding actors to global names for lookup
/// - Managing the global actor system lifecycle
pub struct GlobalContext;

impl GlobalContext {
    /// Get a reference to the global context singleton.
    pub fn get() -> &'static Self {
        &GLOBAL_CONTEXT_INSTANCE
    }

    pub async fn spawn<A: Actor>(&self, args: A::Args) -> ActorRef<A> {
        let (actor_hdl, actor) = spawn_impl(&GLOBAL_HDL, args).await;
        GLOBAL_ROOT_HDLS.lock().unwrap().push(actor_hdl);
        actor
    }

    pub fn bind<A: Actor>(&self, key: impl Into<Cow<'static, str>>, actor: ActorRef<A>) {
        GLOBAL_BINDINGS.lock().unwrap().bind(key, actor);
    }

    pub fn lookup<A: Actor>(&self, key: &str) -> Option<ActorRef<A>> {
        GLOBAL_BINDINGS.lock().unwrap().lookup(key)
    }

    pub fn free<Q>(&self, key: &Q) -> bool
    where
        Q: ?Sized + AsRef<str>,
    {
        GLOBAL_BINDINGS.lock().unwrap().free(key)
    }

    /// Get the number of active root actors.
    pub fn root_actor_count(&self) -> usize {
        GLOBAL_ROOT_HDLS.lock().unwrap().len()
    }

    /// Get the number of bound actors.
    pub fn binding_count(&self) -> usize {
        GLOBAL_BINDINGS.lock().unwrap().len()
    }

    /// Check if a binding exists for the given key.
    pub fn has_binding(&self, key: &str) -> bool {
        GLOBAL_BINDINGS.lock().unwrap().contains_key(key)
    }

    pub fn clear_bindings(&self) {
        GLOBAL_BINDINGS.lock().unwrap().clear();
    }

    /// Get a list of all binding keys.
    pub fn binding_keys(&self) -> Vec<String> {
        GLOBAL_BINDINGS.lock().unwrap().keys()
    }

    /// Spawn and immediately bind an actor in one operation.

    pub async fn spawn_and_bind<A: Actor>(
        &self,
        key: impl Into<Cow<'static, str>>,
        args: A::Args,
    ) -> ActorRef<A> {
        let actor = self.spawn(args).await;
        self.bind(key, actor.clone());
        actor
    }

    // /// Attempt to lookup an actor, spawning it if not found.
    // ///
    // /// # Example
    // ///
    // /// ```rust
    // /// let actor = GlobalContext::get()
    // ///     .lookup_or_spawn("my_service", || MyActorArgs { value: 42 })
    // ///     .await;
    // /// ```
    // pub async fn lookup_or_spawn<A: Actor, F>(&self, key: &str, args_fn: F) -> ActorRef<A>
    // where
    //     F: FnOnce() -> A::Args,
    // {
    //     if let Some(actor) = self.lookup(key) {
    //         actor
    //     } else {
    //         self.spawn_and_bind(key, args_fn()).await
    //     }
    // }
}

// Singleton instance
static GLOBAL_CONTEXT_INSTANCE: GlobalContext = GlobalContext;

// Internal global state
pub(crate) static GLOBAL_HDL: LazyLock<ActorHdl> = LazyLock::new(|| {
    let (sig_tx, _sig_rx) = mpsc::unbounded_channel();
    // todo handle escalation from root actors
    ActorHdl(sig_tx)
});

pub(crate) static GLOBAL_ROOT_HDLS: LazyLock<Mutex<Vec<ActorHdl>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

static GLOBAL_BINDINGS: LazyLock<Mutex<GlobalBindings>> =
    LazyLock::new(|| Mutex::new(GlobalBindings::new()));

// Internal bindings management
struct GlobalBindings(HashMap<Cow<'static, str>, Box<dyn Any + Send>>);

impl GlobalBindings {
    fn new() -> Self {
        GlobalBindings(HashMap::new())
    }

    fn bind<A>(&mut self, key: impl Into<Cow<'static, str>>, actor: ActorRef<A>)
    where
        A: Actor,
    {
        self.0.insert(key.into(), Box::new(actor));
    }

    fn lookup<Q, A>(&self, key: &Q) -> Option<ActorRef<A>>
    where
        A: Actor,
        Q: ?Sized + AsRef<str>,
    {
        self.0
            .get(key.as_ref())
            .and_then(|v| v.downcast_ref::<ActorRef<A>>())
            .cloned()
    }

    fn free<Q>(&mut self, key: &Q) -> bool
    where
        Q: ?Sized + AsRef<str>,
    {
        self.0.remove(key.as_ref()).is_some()
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    fn clear(&mut self) {
        self.0.clear();
    }

    fn keys(&self) -> Vec<String> {
        self.0.keys().map(|k| k.to_string()).collect()
    }
}

// Convenience module for backwards compatibility or direct access
pub mod global {
    use super::*;

    /// Spawn a new root actor with the given arguments.
    pub async fn spawn<A: Actor>(args: A::Args) -> ActorRef<A> {
        GlobalContext::get().spawn(args).await
    }

    /// Bind an actor to a global name for later lookup.
    pub fn bind<A: Actor>(key: impl Into<Cow<'static, str>>, actor: ActorRef<A>) {
        GlobalContext::get().bind(key, actor);
    }

    /// Look up an actor by its global name.
    pub fn lookup<A: Actor>(key: &str) -> Option<ActorRef<A>> {
        GlobalContext::get().lookup(key)
    }

    /// Remove an actor binding by its key.
    pub fn free<Q>(key: &Q) -> bool
    where
        Q: ?Sized + AsRef<str>,
    {
        GlobalContext::get().free(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestActor {
        value: i32,
    }

    impl Actor for TestActor {
        type Args = i32;

        async fn initialize(_ctx: crate::context::Context<Self>, args: &Self::Args) -> Self {
            TestActor { value: *args }
        }
    }

    #[tokio::test]
    async fn test_global_context_spawn() {
        let ctx = GlobalContext::get();
        let actor: ActorRef<TestActor> = ctx.spawn(42).await;

        assert_eq!(ctx.root_actor_count(), 1);
    }

    #[tokio::test]
    async fn test_global_context_bind_and_lookup() {
        let ctx = GlobalContext::get();
        let actor: ActorRef<TestActor> = ctx.spawn(100).await;

        ctx.bind("test_actor", actor);

        assert!(ctx.has_binding("test_actor"));
        assert_eq!(ctx.binding_count(), 1);

        let found_actor: Option<ActorRef<TestActor>> = ctx.lookup("test_actor");
        assert!(found_actor.is_some());
    }

    #[tokio::test]
    async fn test_global_context_free() {
        let ctx = GlobalContext::get();
        let actor: ActorRef<TestActor> = ctx.spawn(200).await;

        ctx.bind("test_free", actor);
        assert!(ctx.has_binding("test_free"));

        let removed = ctx.free("test_free");
        assert!(removed);
        assert!(!ctx.has_binding("test_free"));

        let not_removed = ctx.free("nonexistent");
        assert!(!not_removed);
    }

    #[tokio::test]
    async fn test_spawn_and_bind() {
        let ctx = GlobalContext::get();

        let actor: ActorRef<TestActor> = ctx.spawn_and_bind("spawn_bind_test", 300).await;

        let found: Option<ActorRef<TestActor>> = ctx.lookup("spawn_bind_test");
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn test_lookup_or_spawn() {
        let ctx = GlobalContext::get();

        // First call should spawn

        let actor1: ActorRef<TestActor> = ctx.spawn_and_bind("lazy_actor", 400).await;

        assert!(ctx.has_binding("lazy_actor"));

        // Second call should return existing

        let actor2: ActorRef<TestActor> = ctx.lookup("lazy_actor").unwrap();

        // Should be the same actor (same ID)
        assert_eq!(actor1.id(), actor2.id());
    }

    // #[tokio::test]
    // async fn test_clear_bindings() {
    //     let ctx = GlobalContext::get();

    //     ctx.spawn_and_bind("clear_test_1", 1).await;
    //     ctx.spawn_and_bind("clear_test_2", 2).await;

    //     assert_eq!(ctx.binding_count(), 2);

    //     ctx.clear_bindings();
    //     assert_eq!(ctx.binding_count(), 0);
    // }

    // #[tokio::test]
    // async fn test_binding_keys() {
    //     let ctx = GlobalContext::get();

    //     ctx.clear_bindings(); // Clean slate

    //     ctx.spawn_and_bind("key1", 1).await;
    //     ctx.spawn_and_bind("key2", 2).await;

    //     let keys = ctx.binding_keys();
    //     assert_eq!(keys.len(), 2);
    //     assert!(keys.contains(&"key1".to_string()));
    //     assert!(keys.contains(&"key2".to_string()));
    // }

    #[tokio::test]
    async fn test_backwards_compatibility() {
        // Test the global module functions

        let actor: ActorRef<TestActor> = global::spawn(999).await;
        global::bind("compat_test", actor);

        let found: Option<ActorRef<TestActor>> = global::lookup("compat_test");
        assert!(found.is_some());

        let removed = global::free("compat_test");
        assert!(removed);
    }
}
