use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use theta::{persistence::persistent_actor::ContextExt, prelude::*};
use theta_macros::PersistentActor;
#[cfg(feature = "tracing")]
use tracing::warn;
use url::Url;

use theta::persistence::persistent_actor::PersistentActor;

#[derive(Debug, Clone, PersistentActor)]
#[snapshot(ManagerSnapshot)]
pub struct Manager {
    pub regular_config: String,
    pub sub_actors: HashMap<String, ActorRef<Worker>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerArgs {
    pub regular_config: String,
    pub sub_actors: HashMap<String, Url>,
}

// Custom snapshot type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerSnapshot {
    pub regular_config: String,
    pub sub_actors: HashMap<String, Url>,
}

impl From<&Manager> for ManagerSnapshot {
    fn from(actor: &Manager) -> Self {
        Self {
            regular_config: actor.regular_config.clone(),
            sub_actors: actor
                .sub_actors
                .iter()
                .filter_map(|(name, actor_ref)| {
                    Worker::persistence_key(&actor_ref.downgrade()).map(|url| (name.clone(), url))
                })
                .collect(),
        }
    }
}

impl Into<ManagerArgs> for ManagerSnapshot {
    fn into(self) -> ManagerArgs {
        ManagerArgs {
            regular_config: self.regular_config,
            sub_actors: self.sub_actors,
        }
    }
}

impl Actor for Manager {
    type Args = ManagerArgs;

    async fn initialize(ctx: theta::context::Context<Self>, args: &Self::Args) -> Self {
        let sub_actor_buffer: Arc<Mutex<HashMap<String, ActorRef<Worker>>>> = Default::default();

        let respawn_sub_actors = args
            .sub_actors
            .iter()
            .map(|(name, url)| {
                let sub_actor_buffer = sub_actor_buffer.clone();
                let ctx = ctx.clone();
                async move {
                    if let Ok(sub_actor) = ctx
                        .respawn_or(
                            url.clone(),
                            Worker {
                                config: "default".to_string(),
                            },
                        )
                        .await
                    {
                        sub_actor_buffer
                            .lock()
                            .unwrap()
                            .insert(name.clone(), sub_actor);
                    } else {
                        #[cfg(feature = "tracing")]
                        warn!("Failed to respawn sub-actor for URL: {url}");
                    }
                }
            })
            .collect::<Vec<_>>();

        futures::future::join_all(respawn_sub_actors).await;

        let sub_actors = Arc::try_unwrap(sub_actor_buffer)
            .unwrap()
            .into_inner()
            .unwrap();

        Self {
            regular_config: args.regular_config.clone(),
            sub_actors,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PersistentActor)]
pub struct Worker {
    pub config: String,
}

impl Actor for Worker {
    type Args = Worker;

    fn initialize(
        _ctx: theta::context::Context<Self>,
        args: &Self::Args,
    ) -> impl Future<Output = Self> + Send {
        async move {
            // Initialization logic for SubActor
            Self {
                config: args.config.clone(),
            }
        }
    }
}

impl From<&Worker> for Worker {
    fn from(actor: &Worker) -> Self {
        actor.clone()
    }
}

fn main() {}
