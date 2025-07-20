use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::PersistentActor;
use url::Url;

use theta::persistence::persistent_actor::PersistentActor;

#[derive(Debug, Clone, PersistentActor)]
pub struct ManagerActor {
    pub regular_config: String,
    pub sub_actors: HashMap<String, ActorRef<SubActor>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerActorArgs {
    pub regular_config: String,
    pub sub_actors: HashMap<String, Url>,
}

impl From<&ManagerActor> for ManagerActorArgs {
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

impl Actor for ManagerActor {
    type Args = ManagerActorArgs;
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, _actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let sub_actor_buffer: Arc<Mutex<HashMap<String, ActorRef<SubActor>>>> = Default::default();

        let respawn_sub_actors = args
            .sub_actors
            .into_iter()
            .map(|(name, url)| {
                let sub_actor_buffer = sub_actor_buffer.clone();
                async move {
                    if let Ok(sub_actor) = SubActor::respawn_persistent(url.clone()).await {
                        sub_actor_buffer.lock().unwrap().insert(name, sub_actor);
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

        Ok(Self {
            regular_config: args.regular_config,
            sub_actors,
        })
    }
}

#[derive(Debug, Clone, Actor, Serialize, Deserialize, PersistentActor)]
pub struct SubActor {
    pub config: String,
}

impl From<&SubActor> for SubActor {
    fn from(actor: &SubActor) -> Self {
        actor.clone()
    }
}

fn main() {}
