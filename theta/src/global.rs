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

pub async fn spawn<A: Actor>(args: A::Args) -> ActorRef<A> {
    let (actor_hdl, actor) = spawn_impl(&GLOBAL_HDL, args).await;

    GLOBAL_ROOT_HDLS.lock().unwrap().push(actor_hdl);

    actor
}

pub fn bind<A: Actor>(key: impl Into<Cow<'static, str>>, actor: ActorRef<A>) {
    GLOBAL_BINDINGS.lock().unwrap().bind(key, actor);
}

pub fn lookup<Q, A: Actor>(key: &Q) -> Option<ActorRef<A>>
where
    Q: ?Sized + AsRef<str>,
{
    GLOBAL_BINDINGS.lock().unwrap().lookup(key)
}

pub fn free<Q>(key: &Q) -> bool
where
    Q: ?Sized + AsRef<str>,
{
    GLOBAL_BINDINGS.lock().unwrap().free(key)
}

pub(crate) static GLOBAL_HDL: LazyLock<ActorHdl> = LazyLock::new(|| {
    let (sig_tx, _sig_rx) = mpsc::unbounded_channel();

    // todo handle escalation from root actors

    ActorHdl(sig_tx)
});

pub(crate) static GLOBAL_ROOT_HDLS: LazyLock<Mutex<Vec<ActorHdl>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

static GLOBAL_BINDINGS: LazyLock<Mutex<GlobalBindings>> =
    LazyLock::new(|| Mutex::new(GlobalBindings::new()));

struct GlobalBindings(HashMap<Cow<'static, str>, Box<dyn Any + Send>>);

// Implementations

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
}
