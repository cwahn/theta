use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use theta::{persistence::persistent_actor::ContextExt, prelude::*};
use theta_macros::PersistentActor;
use url::Url;

use theta::persistence::persistent_actor::PersistentActor;

#[derive(Debug, Clone, PersistentActor)]
#[snapshot(ManagerActorSnapshot)]
pub struct ManagerActor {
    pub regular_config: String,
    pub sub_actors: HashMap<String, ActorRef<SubActor>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerActorArgs {
    pub regular_config: String,
    pub sub_actors: HashMap<String, Url>,
}

// Custom snapshot type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerActorSnapshot {
    pub regular_config: String,
    pub sub_actors: HashMap<String, Url>,
}

impl From<&ManagerActor> for ManagerActorSnapshot {
    fn from(actor: &ManagerActor) -> Self {
        Self {
            regular_config: actor.regular_config.clone(),
            sub_actors: actor
                .sub_actors
                .iter()
                .filter_map(|(name, actor_ref)| {
                    PersistentActor::persistence_key(actor_ref).map(|url| (name.clone(), url))
                })
                .collect(),
        }
    }
}

impl Into<ManagerActorArgs> for ManagerActorSnapshot {
    fn into(self) -> ManagerActorArgs {
        ManagerActorArgs {
            regular_config: self.regular_config,
            sub_actors: self.sub_actors,
        }
    }
}

impl Actor for ManagerActor {
    type Args = ManagerActorArgs;

    async fn initialize(ctx: theta::context::Context<Self>, args: &Self::Args) -> Self {
        let sub_actor_buffer: Arc<Mutex<HashMap<String, ActorRef<SubActor>>>> = Default::default();

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
                            SubActor {
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
                        tracing::warn!("Failed to respawn sub-actor for URL: {url}");
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
pub struct SubActor {
    pub config: String,
}

impl Actor for SubActor {
    type Args = SubActor;

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

impl From<&SubActor> for SubActor {
    fn from(actor: &SubActor) -> Self {
        actor.clone()
    }
}

fn main() {}
