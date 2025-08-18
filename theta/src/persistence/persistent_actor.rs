// use anyhow::anyhow;
// #[cfg(not(target_arch = "wasm32"))]
// use anyhow::bail;

use serde::{Deserialize, Serialize};
use std::any;
use std::fmt::Debug;
use thiserror::Error;

use crate::{
    actor::{Actor, ActorArgs, ActorId},
    actor_ref::ActorRef,
    context::{Context, RootContext, spawn_with_id_impl},
    debug,
};

pub trait PersistentStorage: Send + Sync {
    fn try_read(&self, id: ActorId) -> impl Future<Output = Result<Vec<u8>, anyhow::Error>> + Send;
    fn try_write(
        &self,
        id: ActorId,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), anyhow::Error>> + Send;
}

pub trait PersistentActor: Actor {
    type Snapshot: Debug
        + Clone
        + Send
        + Sync
        + Serialize
        + for<'a> Deserialize<'a>
        + for<'a> From<&'a Self>
        + ActorArgs<Actor = Self>;
}

pub trait PersistentSpawnExt {
    /// - ! Since it coerce the given Id to actor, it is callers' responsibility to ensure no more than one actor is spawned with the same ID.
    ///     - Failure to do so will not result error immediately, but latened undefined behavior.
    /// - It does not invoke [`SaveSnapshotExt::save_snapshot`] automatically, it should be specified in [`ActorArgs::initialize`] if needed.
    fn spawn_persistent<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        args: Args,
    ) -> impl Future<Output = Result<ActorRef<Args::Actor>, PersistenceError>> + Send
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor;

    /// - Since it coerce the given Id to actor, it is callers' responsibility to ensure no more than one actor is spawned with the same ID.
    ///     - Failure to do so will not result error immediately, but latened undefined behavior.
    /// - It does not invoke [`SaveSnapshotExt::save_snapshot`] automatically, it should be specified in [`ActorArgs::initialize`] if needed.
    fn respawn<S, B>(
        &self,
        storage: &S,
        actor_id: ActorId,
    ) -> impl Future<Output = Result<ActorRef<B>, PersistenceError>> + Send
    where
        S: PersistentStorage,
        B: Actor + PersistentActor;

    /// - Since it coerce the given Id to actor, it is callers' responsibility to ensure no more than one actor is spawned with the same ID.
    ///     - Failure to do so will not result error immediately, but latened undefined behavior.
    /// - It does not invoke [`SaveSnapshotExt::save_snapshot`] automatically, it should be specified in [`ActorArgs::initialize`] if needed.
    fn respawn_or<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        args: Args,
    ) -> impl Future<Output = Result<ActorRef<Args::Actor>, PersistenceError>> + Send
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor;
}

pub trait SaveSnapshotExt<A>
where
    A: Actor + PersistentActor,
{
    /// - Since the snapshots are managed by actor_id, it is recommended to only call this method for actor with known actor_id.
    ///     - Otherwise, it will likely result stack up garbage data on the storage.
    fn save_snapshot<S: PersistentStorage>(
        &self,
        storage: &S,
        state: &A,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error(transparent)]
    IoError(#[from] anyhow::Error), // Use anyhow because std::io::Error is !UnwindSafe
    #[error(transparent)]
    SerializeError(postcard::Error),
    #[error(transparent)]
    DeserializeError(postcard::Error),
}

// Implementation

impl<A> PersistentSpawnExt for Context<A>
where
    A: Actor,
{
    async fn spawn_persistent<S, Args>(
        &self,
        _storage: &S,
        actor_id: ActorId,
        args: Args,
    ) -> Result<ActorRef<Args::Actor>, PersistenceError>
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor,
    {
        let (hdl, actor) = spawn_with_id_impl(actor_id, &self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(hdl.downgrade());

        Ok(actor)
    }

    async fn respawn<S, B>(
        &self,
        storage: &S,
        actor_id: ActorId,
    ) -> Result<ActorRef<B>, PersistenceError>
    where
        S: PersistentStorage,
        B: Actor + PersistentActor,
    {
        let bytes = storage.try_read(actor_id).await?;

        let snapshot: B::Snapshot =
            postcard::from_bytes(&bytes).map_err(PersistenceError::DeserializeError)?;

        let actor = self.spawn_persistent(storage, actor_id, snapshot).await?;

        Ok(actor)
    }

    async fn respawn_or<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        args: Args,
    ) -> Result<ActorRef<Args::Actor>, PersistenceError>
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor,
    {
        match self.respawn(storage, actor_id).await {
            Ok(actor) => Ok(actor),
            Err(e) => {
                debug!(
                    "Failed to respawn persistent actor {} with ID {actor_id:?}: {e}. Creating a new instance.",
                    any::type_name::<A>(),
                );
                self.spawn_persistent(storage, actor_id, args).await
            }
        }
    }
}

impl PersistentSpawnExt for RootContext {
    async fn spawn_persistent<S, Args>(
        &self,
        _storage: &S,
        actor_id: ActorId,
        args: Args,
    ) -> Result<ActorRef<Args::Actor>, PersistenceError>
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor,
    {
        let (hdl, actor) = spawn_with_id_impl(actor_id, &self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(hdl.downgrade());

        Ok(actor)
    }

    async fn respawn<S, B>(
        &self,
        storage: &S,
        actor_id: ActorId,
    ) -> Result<ActorRef<B>, PersistenceError>
    where
        S: PersistentStorage,
        B: Actor + PersistentActor,
    {
        let bytes = storage.try_read(actor_id).await?;

        let snapshot: B::Snapshot =
            postcard::from_bytes(&bytes).map_err(PersistenceError::DeserializeError)?;

        let actor = self.spawn_persistent(storage, actor_id, snapshot).await?;

        Ok(actor)
    }

    async fn respawn_or<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        args: Args,
    ) -> Result<ActorRef<Args::Actor>, PersistenceError>
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor,
    {
        match self.respawn(storage, actor_id).await {
            Ok(actor) => Ok(actor),
            Err(e) => {
                debug!(
                    "Failed to respawn persistent actor {} with ID {actor_id:?}: {e}. Creating a new instance.",
                    any::type_name::<Args::Actor>(),
                );
                self.spawn_persistent(storage, actor_id, args).await
            }
        }
    }
}

impl<A> SaveSnapshotExt<A> for Context<A>
where
    A: Actor + PersistentActor,
{
    fn save_snapshot<S: PersistentStorage>(
        &self,
        storage: &S,
        state: &A,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        let snapshot = A::Snapshot::from(state);
        let actor_id = self.id();

        async move {
            storage
                .try_write(actor_id, postcard::to_stdvec(&snapshot)?)
                .await?;

            Ok(())
        }
    }
}
