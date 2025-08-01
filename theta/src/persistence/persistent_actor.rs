use anyhow::anyhow;
#[cfg(not(target_arch = "wasm32"))]
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::any;
use std::fmt::Debug;
use tracing::{debug, trace, warn};
use url::Url;
use uuid::Uuid;

use crate::actor::ActorConfig;
use crate::{
    actor::Actor, actor_ref::ActorRef, context::Context, global_context::GlobalContext,
    prelude::WeakActorRef,
};

// todo Make deriving macro for this trait
pub trait PersistentActor: Actor {
    type Snapshot: Debug
        + ActorConfig<Actor = Self>
        + Clone
        + Send
        + Sync
        + Serialize
        + for<'a> Deserialize<'a>
        + for<'a> From<&'a Self>;

    // Per "Actor" unique key for persistent storage
    // One could use other kind of permanent storage, but it should be directory like structure
    // ! Key should be directory path in case of file system
    // todo type Key: Debug + Clone + Hash;

    // todo type Error: Debug + std::error::Error + Send + Sync;

    // Required
    fn bind_persistent(persistence_key: Url, actor: WeakActorRef<Self>) -> anyhow::Result<()>;

    /// Return persistence key if the actor is persistent.
    fn persistence_key(actor: &WeakActorRef<Self>) -> Option<Url>;

    /// Return an existing persistent actor reference if it exists.
    fn lookup_persistent(persistence_key: &Url) -> Option<ActorRef<Self>>;

    /// Save the current state of the actor to the persistent storage.
    fn save_snapshot(
        &self,
        actor: &WeakActorRef<Self>,
    ) -> impl Future<Output = anyhow::Result<()>> {
        Box::pin(async move {
            let Some(key) = Self::persistence_key(actor) else {
                trace!(
                    "Actor {} is not persistent, skipping snapshot save.",
                    any::type_name::<Self>()
                );
                return Ok(());
            };

            let snapshot = Self::Snapshot::from(self);

            Self::try_write(&key, snapshot).await?;

            Ok(())
        })
    }

    /// Try to read the persistent actor's snapshot from the persistent storage.
    fn try_read(persistence_key: &Url) -> impl Future<Output = anyhow::Result<Vec<u8>>> + Send {
        Box::pin(async move {
            match persistence_key.scheme() {
                #[cfg(not(target_arch = "wasm32"))] // Not supported on WASM
                "file" => {
                    let path = persistence_key
                        .to_file_path()
                        .map_err(|_| anyhow!("Failed to convert Url to file path"))?;

                    if !path.exists() {
                        bail!("persistence key does not exist: {path:?}");
                    }

                    Ok(std::fs::read(path.join("index.bin"))?)
                }
                // todo Support http(s), Ws(s), S3, etc.
                _ => Err(anyhow!(
                    "Unsupported scheme for persistence key: {}",
                    persistence_key.scheme()
                )),
            }
        })
    }

    /// Try to write the persistent actor's snapshot to the persistent storage.
    fn try_write(
        persistence_key: &Url,
        snapshot: Self::Snapshot,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        Box::pin(async move {
            debug!(
                "Saving snapshot {snapshot:#?} for actor: {:?} with key: {persistence_key:?}",
                any::type_name::<Self>(),
            );

            let _data = postcard::to_stdvec(&snapshot)?;

            match persistence_key.scheme() {
                #[cfg(not(target_arch = "wasm32"))] // Not supported on WASM
                "file" => {
                    let path = persistence_key
                        .to_file_path()
                        .map_err(|_| anyhow!("Failed to convert Url to file path"))?;

                    if !path.exists() {
                        std::fs::create_dir_all(&path)?;
                    } else if !path.is_dir() {
                        bail!("persistence key exists but is not a directory: {:?}", path);
                    }

                    std::fs::write(path.join("index.bin"), _data)?;

                    Ok(())
                }
                // todo Support http(s), Ws(s), S3, etc.
                _ => Err(anyhow!(
                    "Unsupported scheme for persistencekey: {}",
                    persistence_key.scheme()
                )),
            }
        })
    }
}

pub trait ContextExt<A>
where
    A: Actor + PersistentActor,
{
    /// Spawn a new persistent actor with the given persistence key and configuration.
    fn spawn_persistent(
        &self,
        persistence_key: Url,
        cfg: impl ActorConfig<Actor = A>,
    ) -> impl Future<Output = anyhow::Result<ActorRef<A>>> + Send;

    /// Respawn a persistent actor from the persistent storage.
    fn respawn(
        &self,
        persistence_key: Url,
    ) -> impl Future<Output = anyhow::Result<ActorRef<A>>> + Send;

    /// Try to respawn a persistent actor and create a new instance if it fails.
    fn respawn_or(
        &self,
        persistence_key: Url,
        cfg: impl ActorConfig<Actor = A>,
    ) -> impl Future<Output = anyhow::Result<ActorRef<A>>> + Send;
}

impl<A, B> ContextExt<A> for Context<B>
where
    A: Actor + PersistentActor,
    B: Actor,
{
    async fn spawn_persistent(
        &self,
        persistence_key: Url,
        cfg: impl ActorConfig<Actor = A>,
    ) -> anyhow::Result<ActorRef<A>> {
        let actor = self.spawn(cfg).await;
        A::bind_persistent(persistence_key, actor.downgrade())?;
        Ok(actor)
    }

    async fn respawn(&self, persistence_key: Url) -> anyhow::Result<ActorRef<A>> {
        if let Some(actor) = A::lookup_persistent(&persistence_key) {
            trace!(
                "Found existing persistent actor {} with key {persistence_key:?}.",
                any::type_name::<A>(),
            );
            return Ok(actor);
        }

        let data = A::try_read(&persistence_key).await?;

        let snapshot: <A as PersistentActor>::Snapshot = postcard::from_bytes(&data)?;

        let actor = self.spawn_persistent(persistence_key, snapshot).await?;

        Ok(actor)
    }

    async fn respawn_or(
        &self,
        persistence_key: Url,
        cfg: impl ActorConfig<Actor = A>,
    ) -> anyhow::Result<ActorRef<A>> {
        match <Context<B> as ContextExt<A>>::respawn(self, persistence_key.clone()).await {
            Ok(actor_ref) => Ok(actor_ref),
            Err(_e) => {
                warn!(
                    "Failed to respawn persistent actor {} with key {persistence_key:?}: {_e}. Creating a new instance.",
                    any::type_name::<A>(),
                );
                self.spawn_persistent(persistence_key, cfg).await
            }
        }
    }
}

impl<A> ContextExt<A> for GlobalContext
where
    A: Actor + PersistentActor,
{
    async fn spawn_persistent(
        &self,
        persistence_key: Url,
        cfg: impl ActorConfig<Actor = A>,
    ) -> anyhow::Result<ActorRef<A>> {
        let actor = self.spawn(cfg).await;
        A::bind_persistent(persistence_key, actor.downgrade())?;
        Ok(actor)
    }

    async fn respawn(&self, persistence_key: Url) -> anyhow::Result<ActorRef<A>> {
        if let Some(actor) = A::lookup_persistent(&persistence_key) {
            trace!(
                "Found existing persistent actor {} with key {persistence_key:?}.",
                any::type_name::<A>(),
            );
            return Ok(actor);
        }

        let data = A::try_read(&persistence_key).await?;

        let snapshot: <A as PersistentActor>::Snapshot = postcard::from_bytes(&data)?;

        let actor = self.spawn_persistent(persistence_key, snapshot).await?;

        Ok(actor)
    }

    async fn respawn_or(
        &self,
        persistence_key: Url,
        cfg: impl ActorConfig<Actor = A>,
    ) -> anyhow::Result<ActorRef<A>> {
        match <GlobalContext as ContextExt<A>>::respawn(self, persistence_key.clone()).await {
            Ok(actor_ref) => Ok(actor_ref),
            Err(_e) => {
                warn!(
                    "Failed to respawn persistent actor {} with key {persistence_key:?}: {_e}. Creating a new instance.",
                    any::type_name::<A>(),
                );
                self.spawn_persistent(persistence_key, cfg).await
            }
        }
    }
}

const fn mix(lhs: Uuid, rhs: Uuid) -> Uuid {
    Uuid::from_u128(lhs.as_u128() ^ rhs.as_u128())
}
