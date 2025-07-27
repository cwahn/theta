// use std::{
//     collections::HashMap,
//     sync::{Arc, Mutex},
// };

// use serde::{Deserialize, Serialize};
// use theta::{actor::ActorConfig, persistence::persistent_actor::ContextExt, prelude::*};
// use theta_macros::PersistentActor;
// use tracing::error;
// use tracing::warn;
// use url::Url;

// use theta::persistence::persistent_actor::PersistentActor;

// #[derive(Debug, Clone, Actor, PersistentActor)]
// #[snapshot(ManagerConfig)]
// pub struct Manager {
//     pub regular_config: String,
//     pub sub_actors: HashMap<String, ActorRef<Worker>>,
// }

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct ManagerConfig {
//     pub regular_config: String,
//     pub sub_actors: HashMap<String, Url>,
// }

// impl ActorConfig for ManagerConfig {
//     type Actor = Manager;

//     async fn initialize(ctx: Context<Self::Actor>, cfg: &Self) -> Self::Actor {
//         let sub_actor_buffer: Arc<Mutex<HashMap<String, ActorRef<Worker>>>> = Default::default();

//         // Spawn each future as a separate task - JoinHandle<T> is UnwindSafe
//         let handles: Vec<_> = cfg
//             .sub_actors
//             .iter()
//             .map(|(name, url)| {
//                 let sub_actor_buffer = sub_actor_buffer.clone();
//                 let ctx = ctx.clone();
//                 let name = name.clone();
//                 let url = url.clone();

//                 tokio::spawn(async move {
//                     if let Ok(sub_actor) = ctx
//                         .respawn_or(
//                             url.clone(),
//                             Worker {
//                                 config: "default".to_string(),
//                             },
//                         )
//                         .await
//                     {
//                         sub_actor_buffer.lock().unwrap().insert(name, sub_actor);
//                     } else {
//                         warn!("Failed to respawn sub-actor for URL: {url}");
//                     }
//                 })
//             })
//             .collect();

//         // Wait for all spawned tasks to complete - this is UnwindSafe
//         // todo Need to find UnwindSafe parallel execution
//         for handle in handles {
//             if let Err(e) = handle.await {
//                 error!("Task panicked: {:?}", e);
//             }
//         }

//         let sub_actors = Arc::try_unwrap(sub_actor_buffer)
//             .unwrap()
//             .into_inner()
//             .unwrap();

//         Self::Actor {
//             regular_config: cfg.regular_config.clone(),
//             sub_actors,
//         }
//     }
// }

// impl From<&Manager> for ManagerConfig {
//     fn from(actor: &Manager) -> Self {
//         Self {
//             regular_config: actor.regular_config.clone(),
//             sub_actors: actor
//                 .sub_actors
//                 .iter()
//                 .filter_map(|(name, actor_ref)| {
//                     Worker::persistence_key(&actor_ref.downgrade()).map(|url| (name.clone(), url))
//                 })
//                 .collect(),
//         }
//     }
// }

// #[derive(Debug, Clone, Serialize, Deserialize, PersistentActor)]
// pub struct Worker {
//     pub config: String,
// }

// impl Actor for Worker {}

// impl ActorConfig for Worker {
//     // type Args = Worwker;
//     type Actor = Self;

//     async fn initialize(
//         _ctx: theta::context::Context<Self>,
//         // args: &Self::Args,
//         cfg: &Self,
//     ) -> Self {
//         // Initialization logic for SubActor
//         Self {
//             config: cfg.config.clone(),
//         }
//     }
// }

// impl From<&Worker> for Worker {
//     fn from(actor: &Worker) -> Self {
//         actor.clone()
//     }
// }

// fn main() {}
