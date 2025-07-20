use anyhow::anyhow;
use serde::{Deserialize, Serialize};
#[cfg(feature = "tracing")]
use std::any;
#[cfg(feature = "tracing")]
use std::fmt::Debug;
#[cfg(feature = "tracing")]
use tracing::{debug, trace, warn};
use url::Url;

use crate::{actor::Actor, actor_ref::ActorRef, context::Context, global::GlobalContext};

// todo Make deriving macro for this trait
pub trait PersistentActor: Actor {
    #[cfg(feature = "tracing")]
    type Snapshot: Debug
        + Clone
        + Send
        + Sync
        + Serialize
        + for<'a> Deserialize<'a>
        + Into<<Self as Actor>::Args>
        + for<'a> From<&'a Self>;
    // + for<'a> TryFrom<&'a Url>; // Usually Self::Args

    #[cfg(not(feature = "tracing"))]
    type Snapshot: Clone
        + Send
        + Sync
        + Serialize
        + for<'a> Deserialize<'a>
        + Into<<Self as Actor>::Args>
        + for<'a> From<&'a Self>;

    /// Per "Actor" unique key for persistent storage
    // One could use other kind of permanent storage, but it should be directory like structure
    // ! Key should be directory path in case of file system
    // todo type Key: Debug + Clone + Hash;

    // todo type Error: Debug + std::error::Error + Send + Sync;

    // Recommaned to be implemented with
    // static REGIESTRY: LazyLock<RwLock<BiMap<Url, WeakActorRef<Self>>>> =
    // LazyLock::new(|| RwLock::new(BiMap::new()));

    // Required
    fn bind_persistent(persistence_key: Url, actor: &ActorRef<Self>) -> anyhow::Result<()>;

    /// Return persistence key if the actor is persistent.
    fn persistence_key(actor: &ActorRef<Self>) -> Option<Url>;

    /// Return an existing persistent actor reference if it exists.
    fn lookup_persistent(persistence_key: &Url) -> Option<ActorRef<Self>>;

    /// Save the current state of the actor to the persistent storage.
    fn save_snapshot(&self, actor: &ActorRef<Self>) -> impl Future<Output = anyhow::Result<()>> {
        Box::pin(async move {
            let Some(key) = Self::persistence_key(actor) else {
                #[cfg(feature = "tracing")]
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
    fn try_read(persistence_key: &Url) -> impl Future<Output = anyhow::Result<Vec<u8>>> {
        Box::pin(async move {
            match persistence_key.scheme() {
                "file" => {
                    let path = persistence_key
                        .to_file_path()
                        .map_err(|_| anyhow!("Failed to convert Url to file path"))?;

                    if !path.exists() {
                        anyhow::bail!("persistence key does not exist: {path:?}");
                    }

                    Ok(std::fs::read(&path.join("index.bin"))?)
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
    ) -> impl Future<Output = anyhow::Result<()>> {
        Box::pin(async move {
            #[cfg(feature = "tracing")]
            debug!(
                "Saving snapshot {snapshot:#?} for actor: {:?} with key: {persistence_key:?}",
                any::type_name::<Self>(),
            );

            let data = postcard::to_stdvec(&snapshot)?;

            match persistence_key.scheme() {
                "file" => {
                    let path = persistence_key
                        .to_file_path()
                        .map_err(|_| anyhow!("Failed to convert Url to file path"))?;

                    if !path.exists() {
                        std::fs::create_dir_all(&path)?;
                    } else if !path.is_dir() {
                        anyhow::bail!("persistence key exists but is not a directory: {:?}", path);
                    }

                    std::fs::write(&path.join("index.bin"), data)?;

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

pub trait ContextExt<B>
where
    B: Actor + PersistentActor,
{
    /// Spawn a new persistent actor with the given arguments.
    fn spawn_persistent(
        &self,
        persistence_key: Url,
        args: <B as Actor>::Args,
    ) -> impl Future<Output = anyhow::Result<ActorRef<B>>>;

    /// Respawn a persistent actor from the persistent storage.
    fn respawn(&self, persistence_key: Url) -> impl Future<Output = anyhow::Result<ActorRef<B>>>;

    /// Try to respawn a persistent actor and create a new instance if it fails.
    fn respawn_or(
        &self,
        persistence_key: Url,
        args: <B as Actor>::Args,
    ) -> impl Future<Output = anyhow::Result<ActorRef<B>>>;
}

// Implementations

impl<A, B> ContextExt<B> for Context<A>
where
    A: Actor,
    B: Actor + PersistentActor,
{
    async fn spawn_persistent(
        &self,
        persistence_key: Url,
        args: <B as Actor>::Args,
    ) -> anyhow::Result<ActorRef<B>> {
        let actor = self.spawn::<B>(args).await;

        B::bind_persistent(persistence_key, &actor)?;

        Ok(actor)
    }

    async fn respawn(&self, persistence_key: Url) -> anyhow::Result<ActorRef<B>> {
        if let Some(actor) = B::lookup_persistent(&persistence_key) {
            #[cfg(feature = "tracing")]
            trace!(
                "Found existing persistent actor {} with key {persistence_key:?}.",
                any::type_name::<B>(),
            );
            return Ok(actor);
        }

        let data = B::try_read(&persistence_key).await?;
        let snapshot: B::Snapshot = postcard::from_bytes(&data)?;

        let actor = self
            .spawn_persistent(persistence_key, snapshot.into())
            .await?;

        Ok(actor)
    }

    async fn respawn_or(
        &self,
        persistence_key: Url,
        args: <B as Actor>::Args,
    ) -> anyhow::Result<ActorRef<B>> {
        match self.respawn(persistence_key.clone()).await {
            Ok(actor_ref) => Ok(actor_ref),
            Err(_e) => {
                #[cfg(feature = "tracing")]
                warn!(
                    "Failed to respawn persistent actor {} with key {persistence_key:?}: {_e}. Creating a new instance.",
                    any::type_name::<B>(),
                );
                self.spawn_persistent(persistence_key, args).await
            }
        }
    }
}

impl<B> ContextExt<B> for GlobalContext
where
    B: Actor + PersistentActor,
{
    async fn spawn_persistent(
        &self,
        persistence_key: Url,
        args: <B as Actor>::Args,
    ) -> anyhow::Result<ActorRef<B>> {
        let actor = self.spawn::<B>(args).await;

        B::bind_persistent(persistence_key, &actor)?;

        Ok(actor)
    }

    async fn respawn(&self, persistence_key: Url) -> anyhow::Result<ActorRef<B>> {
        if let Some(actor) = B::lookup_persistent(&persistence_key) {
            #[cfg(feature = "tracing")]
            trace!(
                "Found existing persistent actor {} with key {persistence_key:?}.",
                any::type_name::<B>(),
            );
            return Ok(actor);
        }

        let data = B::try_read(&persistence_key).await?;
        let snapshot: B::Snapshot = postcard::from_bytes(&data)?;

        let actor = self
            .spawn_persistent(persistence_key, snapshot.into())
            .await?;

        Ok(actor)
    }

    async fn respawn_or(
        &self,
        persistence_key: Url,
        args: <B as Actor>::Args,
    ) -> anyhow::Result<ActorRef<B>> {
        match self.respawn(persistence_key.clone()).await {
            Ok(actor_ref) => Ok(actor_ref),
            Err(_e) => {
                #[cfg(feature = "tracing")]
                warn!(
                    "Failed to respawn persistent actor {} with key {persistence_key:?}: {_e}. Creating a new instance.",
                    any::type_name::<B>(),
                );
                self.spawn_persistent(persistence_key, args).await
            }
        }
    }
}
