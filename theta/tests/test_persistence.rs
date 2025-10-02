#![cfg(all(feature = "persistence", feature = "macros", feature = "project_dir"))]

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Once},
    time::Duration,
};
use tempfile::TempDir;
use theta::{
    actor::{Actor, ActorArgs, ActorId},
    context::{Context, RootContext},
    persistence::{
        persistent_actor::{PersistentContextExt, PersistentSpawnExt},
        storages::project_dir::LocalFs,
    },
    prelude::ActorRef,
};
use theta_macros::{ActorArgs, actor};
use tokio::time::sleep;
use tracing::warn;
use uuid::uuid;

// Global initialization for tests
static INIT: Once = Once::new();
static TEMP_DIR: Mutex<Option<TempDir>> = Mutex::new(None);

fn ensure_localfs_init() {
    INIT.call_once(|| {
        let temp_dir = TempDir::new().unwrap();
        LocalFs::init(temp_dir.path());
        *TEMP_DIR.lock().unwrap() = Some(temp_dir);
    });
}

// Test actors
#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
pub struct Counter {
    pub count: i32,
}

// Messages for CounterActor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Increment(pub i32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCount;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Save;

#[actor("847d1a75-bf42-4690-b947-c3f206fda4cf", snapshot = Counter)]
impl Actor for Counter {
    const _: () = {
        async |Increment(value): Increment| {
            self.count += value;
            let _ = ctx.save_snapshot(&LocalFs, self).await;
        };

        async |_: GetCount| -> i32 { self.count };

        async |_: Save| {
            let _ = ctx.save_snapshot(&LocalFs, self).await;
        };
    };
}

#[derive(Debug, Clone)]
pub struct Manager {
    pub config: String,
    pub counters: HashMap<String, ActorRef<Counter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerArgs {
    pub config: String,
    pub counter_ids: HashMap<String, ActorId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCounter {
    pub name: String,
}

impl ActorArgs for ManagerArgs {
    type Actor = Manager;

    async fn initialize(ctx: Context<Self::Actor>, cfg: &Self) -> Self::Actor {
        let counter_buffer: Arc<Mutex<HashMap<String, ActorRef<Counter>>>> = Default::default();

        let respawn_tasks = cfg
            .counter_ids
            .iter()
            .map(|(name, id)| {
                let counter_buffer = counter_buffer.clone();
                let ctx = ctx.clone();
                let name = name.clone();
                let id = id.clone();

                async move {
                    if let Ok(counter) = ctx
                        .respawn_or(&LocalFs, id.clone(), || (), || Counter { count: 0 })
                        .await
                    {
                        counter_buffer.lock().unwrap().insert(name, counter);
                    } else {
                        warn!("failed to respawn counter for Id: {id}");
                    }
                }
            })
            .collect::<Vec<_>>();

        // todo Need to find UnwindSafe parallel execution
        for task in respawn_tasks {
            task.await;
        }

        let counters = Arc::try_unwrap(counter_buffer)
            .unwrap()
            .into_inner()
            .unwrap();

        Self::Actor {
            config: cfg.config.clone(),
            counters,
        }
    }
}

#[actor("4397a912-188c-45ea-8a3d-6c4ebef95911", snapshot = ManagerArgs)]
impl Actor for Manager {
    const _: () = {
        async |GetCounter { name }: GetCounter| -> Option<ActorRef<Counter>> {
            self.counters.get(&name).cloned()
        };
    };
}

impl From<&Manager> for ManagerArgs {
    fn from(actor: &Manager) -> Self {
        Self {
            config: actor.config.clone(),
            counter_ids: actor
                .counters
                .iter()
                .map(|(name, actor)| (name.clone(), actor.id()))
                .collect(),
        }
    }
}

#[tokio::test]
async fn test_simple_persistent_actor() {
    ensure_localfs_init();

    let actor_id = uuid::uuid!("9714394b-1dfe-4e2a-9f97-e19272150546");

    // Create a root context (you'll need to adapt this to your actual context creation)
    let ctx = RootContext::init_local();

    // Test 1: Create a new persistent actor
    let counter = ctx
        .spawn_persistent(&LocalFs, actor_id, Counter { count: 5 })
        .await
        .unwrap();

    // Verify initial state
    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 5);

    // Modify state
    counter.tell(Increment(10)).unwrap();
    sleep(Duration::from_millis(10)).await; // Let the message process

    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 15);

    // Drop the actor reference
    drop(counter);

    // Test 2: Respawn the actor from persistence
    let respawned_counter: ActorRef<Counter> = ctx.respawn(&LocalFs, actor_id, ()).await.unwrap();

    // Verify state was restored
    let count = respawned_counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 15);
}

#[tokio::test]
async fn test_respawn_or_fallback() {
    ensure_localfs_init();

    let actor_id = uuid!("02efdacf-aa39-48bc-9750-43cf4c96b9ba");

    let ctx = RootContext::init_local();

    // Test respawn_or with non-existent persistence
    let counter = ctx
        .respawn_or(&LocalFs, actor_id, || (), || Counter { count: 100 })
        .await
        .unwrap();

    // Should create new actor with fallback args
    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 100);

    // Verify it was registered as persistent
    // let persistence_key = Counter::persistence_key(&counter.downgrade());
    // assert!(persistence_key.is_some());
    assert_eq!(counter.id(), actor_id);
}

#[tokio::test]
async fn test_manager_with_persistent_children() {
    ensure_localfs_init();

    let manager_id = uuid!("18fa37e7-a10c-4c6c-8022-132c125cde21");
    let counter1_id = uuid!("fe5f6f30-0b49-4fc8-8db8-0522ab8fc0ee");
    let counter2_id = uuid!("4a525d16-3f57-4d18-9fd8-934061b9351e");

    let ctx = RootContext::init_local();

    // Create some persistent counters first
    let counter1: ActorRef<Counter> = ctx
        .spawn_persistent(&LocalFs, counter1_id, Counter { count: 10 })
        .await
        .unwrap();

    let counter2: ActorRef<Counter> = ctx
        .spawn_persistent(&LocalFs, counter2_id, Counter { count: 20 })
        .await
        .unwrap();

    let _ = counter1.ask(Save).await;
    let _ = counter2.ask(Save).await;

    drop(counter1);
    drop(counter2);

    // Create manager with references to these counters
    let mut counter_ids = HashMap::new();
    counter_ids.insert("c1".to_string(), counter1_id);
    counter_ids.insert("c2".to_string(), counter2_id);

    let manager = ctx
        .spawn_persistent(
            &LocalFs,
            manager_id,
            ManagerArgs {
                config: "test_manager".to_string(),
                counter_ids,
            },
        )
        .await
        .unwrap();

    let counter_1 = manager
        .ask(GetCounter {
            name: "c1".to_string(),
        })
        .await
        .unwrap()
        .unwrap();

    let counter_2 = manager
        .ask(GetCounter {
            name: "c2".to_string(),
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(counter_1.id(), counter1_id);
    assert_eq!(counter_2.id(), counter2_id);

    let count_1 = counter_1.ask(GetCount).await.unwrap();
    let count_2 = counter_2.ask(GetCount).await.unwrap();

    assert_eq!(count_1, 10);
    assert_eq!(count_2, 20);
}

#[tokio::test]
async fn test_persistence_file_operations() {
    ensure_localfs_init();

    let actor_id = uuid!("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");

    let ctx = RootContext::init_local();

    // Test snapshot creation and persistence by creating an actor
    let counter = ctx
        .spawn_persistent(&LocalFs, actor_id, Counter { count: 999 })
        .await
        .unwrap();

    // Verify initial state
    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 999);

    // Modify and save state
    counter.tell(Increment(1)).unwrap();
    sleep(Duration::from_millis(10)).await; // Let the increment and save process

    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 1000);

    // Drop the actor
    drop(counter);

    // Read snapshot back by respawning
    let restored_counter: ActorRef<Counter> = ctx.respawn(&LocalFs, actor_id, ()).await.unwrap();

    let restored_count = restored_counter.ask(GetCount).await.unwrap();
    assert_eq!(restored_count, 1000);
}
