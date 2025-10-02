use std::{any::type_name, fmt::Debug, future::Future};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

use crate::{
    actor::{Actor, ActorArgs, ActorId},
    actor_ref::ActorRef,
    context::{Context, RootContext, spawn_with_id_impl},
};

/// Trait for storage backends that can persist actor state.
///
/// Implementations provide read/write operations for actor snapshots
/// to various storage systems (filesystem, S3, databases, etc.).
pub trait PersistentStorage: Send + Sync {
    /// Try to read a snapshot for the given actor ID.
    fn try_read(&self, id: ActorId) -> impl Future<Output = Result<Vec<u8>, anyhow::Error>> + Send;

    /// Try to write a snapshot for the given actor ID.
    fn try_write(
        &self,
        id: ActorId,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), anyhow::Error>> + Send;
}

/// Trait for actors that support state persistence.
///
/// Persistent actors can save snapshots of their state and be recovered
/// from those snapshots. The snapshot type must implement all necessary
/// traits for serialization and actor initialization.
///
/// # Example
///
/// ```
/// use theta::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
/// struct Counter {
///     value: i64,
/// }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Increment(i64);
///
/// // The `snapshot` flag automatically implements PersistentActor
/// // with Snapshot = Counter, RuntimeArgs = (), ActorArgs = Counter
/// #[actor("12345678-1234-5678-9abc-123456789abc", snapshot)]
/// impl Actor for Counter {
///     const _: () = {
///         async |Increment(amount): Increment| {
///             self.value += amount;
///         };
///     };
/// }
/// ```
pub trait PersistentActor: Actor {
    /// The snapshot type that captures this actor's persistable state.
    ///
    /// This type must:
    /// - Be serializable (`Serialize` + `Deserialize`)
    /// - Convert from the actor state (`From<&Self>`)
    /// - Be usable as actor initialization arguments (`ActorArgs<Actor = Self>`)
    type Snapshot: Debug
        + Clone
        + Send
        + Sync
        + Serialize
        + for<'a> Deserialize<'a>
        + for<'a> From<&'a Self>;

    /// Runtime arguments passed to `persistent_args` along with the snapshot.
    ///
    /// For actors using the `snapshot` macro attribute, this defaults to `()`.
    type RuntimeArgs: Debug + Send;

    /// Actor arguments type used to initialize the actor.
    ///
    /// For actors using the `snapshot` macro attribute without value, it is `Actor` itself.
    /// For actors using `snapshot` with a some type implementing `ActorArgs`, it is that type.
    type ActorArgs: ActorArgs<Actor = Self>;

    /// Combine a snapshot and runtime arguments to create actor arguments.
    ///
    /// For actors using the `snapshot` macro attribute, this simply returns the snapshot
    /// since `RuntimeArgs = ()` and `ActorArgs = Snapshot`.
    fn persistent_args(
        snapshot: Self::Snapshot,
        runtime_args: Self::RuntimeArgs,
    ) -> Self::ActorArgs;
}

/// Extension trait for spawning persistent actors.
pub trait PersistentSpawnExt {
    /// Spawn an actor with persistence support.
    ///
    /// This method will:
    /// 1. Try to load an existing snapshot from storage
    /// 2. If found, use it to restore the actor state
    /// 3. If not found, initialize the actor with the provided arguments
    ///
    /// **Warning**: The caller must ensure no more than one actor is spawned
    /// with the same ID, as this can lead to undefined behavior.
    ///
    /// **Note**: This method does not automatically save snapshots. If you need
    /// automatic snapshots, implement the saving logic in `ActorArgs::initialize`.
    ///
    /// # Arguments
    ///
    /// * `storage` - The storage backend to use for persistence
    /// * `actor_id` - Unique ID for this actor instance (used as storage key)
    /// * `args` - Initialization arguments (used if no snapshot exists)
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
    /// - It does not invoke [`PersistentContextExt::save_snapshot`] automatically, it should be specified in [`ActorArgs::initialize`] if needed.
    fn respawn<S, B>(
        &self,
        storage: &S,
        actor_id: ActorId,
        runtime_args: B::RuntimeArgs,
    ) -> impl Future<Output = Result<ActorRef<B>, PersistenceError>> + Send
    where
        S: PersistentStorage,
        B: Actor + PersistentActor;

    /// - Since it coerce the given Id to actor, it is callers' responsibility to ensure no more than one actor is spawned with the same ID.
    ///     - Failure to do so will not result error immediately, but latened undefined behavior.
    /// - It does not invoke [`PersistentContextExt::save_snapshot`] automatically, it should be specified in [`ActorArgs::initialize`] if needed.
    fn respawn_or<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        runtime_args: impl FnOnce() -> <<Args as ActorArgs>::Actor as PersistentActor>::RuntimeArgs
        + Send,
        default_args: impl FnOnce() -> Args + Send,
    ) -> impl Future<Output = Result<ActorRef<Args::Actor>, PersistenceError>> + Send
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor;
}

/// Extension trait for manually saving actor snapshots to storage.
pub trait PersistentContextExt<A>
where
    A: Actor + PersistentActor,
{
    /// Save a snapshot of the actor's current state to storage.
    ///
    /// This method should only be called for actors with known IDs to avoid
    /// accumulating garbage data in storage.
    fn save_snapshot<S: PersistentStorage>(
        &self,
        storage: &S,
        state: &A,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
}

/// Errors that can occur during persistence operations.
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
        runtime_args: B::RuntimeArgs,
    ) -> Result<ActorRef<B>, PersistenceError>
    where
        S: PersistentStorage,
        B: Actor + PersistentActor,
    {
        let bytes = storage.try_read(actor_id).await?;

        let snapshot: B::Snapshot =
            postcard::from_bytes(&bytes).map_err(PersistenceError::DeserializeError)?;

        let args = B::persistent_args(snapshot, runtime_args);

        let actor = self.spawn_persistent(storage, actor_id, args).await?;

        Ok(actor)
    }

    async fn respawn_or<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        runtime_args: impl FnOnce() -> <<Args as ActorArgs>::Actor as PersistentActor>::RuntimeArgs
        + Send,
        default_args: impl FnOnce() -> Args + Send,
    ) -> Result<ActorRef<Args::Actor>, PersistenceError>
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor,
    {
        let bytes = match storage.try_read(actor_id).await {
            Err(err) => {
                debug!(
                    actor = format_args!("{}({actor_id})",
                    type_name::<<Args as ActorArgs>::Actor>()),
                    %err,
                    "failed to read snapshot"
                );
                return self
                    .spawn_persistent(storage, actor_id, default_args())
                    .await;
            }
            Ok(bytes) => bytes,
        };

        let snapshot: <Args::Actor as PersistentActor>::Snapshot =
            match postcard::from_bytes(&bytes) {
                Err(err) => {
                    warn!(
                        actor = format_args!("{}({actor_id})",
                        type_name::<<Args as ActorArgs>::Actor>()),
                        %err,
                        "failed to deserialize snapshot"
                    );
                    return self
                        .spawn_persistent(storage, actor_id, default_args())
                        .await;
                }
                Ok(snapshot) => snapshot,
            };

        let args = Args::Actor::persistent_args(snapshot, runtime_args());

        self.spawn_persistent(storage, actor_id, args).await
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
        runtime_args: B::RuntimeArgs,
    ) -> Result<ActorRef<B>, PersistenceError>
    where
        S: PersistentStorage,
        B: Actor + PersistentActor,
    {
        let bytes = storage.try_read(actor_id).await?;

        let snapshot: B::Snapshot =
            postcard::from_bytes(&bytes).map_err(PersistenceError::DeserializeError)?;

        let args = B::persistent_args(snapshot, runtime_args);

        let actor = self.spawn_persistent(storage, actor_id, args).await?;

        Ok(actor)
    }

    async fn respawn_or<S, Args>(
        &self,
        storage: &S,
        actor_id: ActorId,
        runtime_args: impl FnOnce() -> <<Args as ActorArgs>::Actor as PersistentActor>::RuntimeArgs,
        default_args: impl FnOnce() -> Args,
    ) -> Result<ActorRef<Args::Actor>, PersistenceError>
    where
        S: PersistentStorage,
        Args: ActorArgs,
        <Args as ActorArgs>::Actor: PersistentActor,
    {
        let bytes = match storage.try_read(actor_id).await {
            Err(err) => {
                debug!(
                    actor = format_args!("{}({actor_id})",
                    type_name::<<Args as ActorArgs>::Actor>()),
                    %err,
                    "failed to read snapshot"
                );
                return self
                    .spawn_persistent(storage, actor_id, default_args())
                    .await;
            }
            Ok(bytes) => bytes,
        };

        let snapshot: <Args::Actor as PersistentActor>::Snapshot =
            match postcard::from_bytes(&bytes) {
                Err(err) => {
                    warn!(
                        actor = format_args!("{}({actor_id})",
                        type_name::<<Args as ActorArgs>::Actor>()),
                        %err,
                        "failed to deserialize snapshot"
                    );
                    return self
                        .spawn_persistent(storage, actor_id, default_args())
                        .await;
                }
                Ok(snapshot) => snapshot,
            };

        let args = Args::Actor::persistent_args(snapshot, runtime_args());

        self.spawn_persistent(storage, actor_id, args).await
    }
}

impl<A> PersistentContextExt<A> for Context<A>
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
