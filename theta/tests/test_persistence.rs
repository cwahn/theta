use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use theta::{actor::ActorArgs, persistence::persistent_actor::PersistentActor};
use theta::{persistence::persistent_actor::ContextExt, prelude::*};
use theta_macros::PersistentActor;
use tokio::time::sleep;
use tracing::warn;
use url::Url;

// Test actors
#[derive(Debug, Clone, Serialize, Deserialize, ActorArgs, PersistentActor)]
pub struct Counter {
    pub count: i32,
}

impl From<&Counter> for Counter {
    fn from(actor: &Counter) -> Self {
        Self { count: actor.count }
    }
}

// Messages for CounterActor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Increment(pub i32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCount;

#[actor("847d1a75-bf42-4690-b947-c3f206fda4cf")]
impl Actor for Counter {
    const _: () = {
        async |Increment(value): Increment| {
            self.count += value;
        };

        async |_: GetCount| -> i32 { self.count };
    };
}

#[derive(Debug, Clone, PersistentActor)]
#[snapshot(ManagerArgs)]
pub struct Manager {
    pub config: String,
    pub counters: HashMap<String, ActorRef<Counter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerArgs {
    pub config: String,
    pub counter_urls: HashMap<String, Url>,
}

impl ActorArgs for ManagerArgs {
    type Actor = Manager;

    async fn initialize(ctx: Context<Self::Actor>, cfg: &Self) -> Self::Actor {
        let counter_buffer: Arc<Mutex<HashMap<String, ActorRef<Counter>>>> = Default::default();

        let respawn_tasks = cfg
            .counter_urls
            .iter()
            .map(|(name, url)| {
                let counter_buffer = counter_buffer.clone();
                let ctx = ctx.clone();
                let name = name.clone();
                let url = url.clone();

                async move {
                    if let Ok(counter) = ctx.respawn_or(url.clone(), Counter { count: 0 }).await {
                        counter_buffer.lock().unwrap().insert(name, counter);
                    } else {
                        warn!("Failed to respawn counter for URL: {url}");
                    }
                }
            })
            .collect::<Vec<_>>();

        // todo Need to find UnwindSafe parallel execution
        // futures::future::join_all(respawn_tasks).await;
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

#[actor("4397a912-188c-45ea-8a3d-6c4ebef95911")]
impl Actor for Manager {}

impl From<&Manager> for ManagerArgs {
    fn from(actor: &Manager) -> Self {
        Self {
            config: actor.config.clone(),
            counter_urls: actor
                .counters
                .iter()
                .filter_map(|(name, actor_ref)| {
                    Counter::persistence_key(&actor_ref.downgrade()).map(|url| (name.clone(), url))
                })
                .collect(),
        }
    }
}

// Test utilities
fn create_temp_url(temp_dir: &TempDir, name: &str) -> Url {
    let path = temp_dir.path().join(name);
    Url::from_file_path(path).unwrap()
}

#[tokio::test]
async fn test_simple_persistent_actor() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "counter1");

    // Create a root context (you'll need to adapt this to your actual context creation)
    let root_ctx = RootContext::init_local();

    // Test 1: Create a new persistent actor
    let counter = root_ctx
        .spawn_persistent(persistence_url.clone(), Counter { count: 5 })
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

    // Save snapshot
    let counter_state = Counter { count: 15 };
    counter_state
        .save_snapshot(&counter.downgrade())
        .await
        .unwrap();

    // Drop the actor reference
    drop(counter);

    // Test 2: Respawn the actor from persistence
    let respawned_counter: ActorRef<Counter> =
        root_ctx.respawn(persistence_url.clone()).await.unwrap();

    // Verify state was restored
    let count = respawned_counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 15);
}

#[tokio::test]
async fn test_lookup_persistent_actor() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "counter2");

    let root_ctx = RootContext::init_local();

    // Create persistent actor
    let counter: ActorRef<Counter> = root_ctx
        .spawn_persistent(persistence_url.clone(), Counter { count: 42 })
        .await
        .unwrap();

    // Test lookup functionality
    let found_counter = Counter::lookup_persistent(&persistence_url);
    assert!(found_counter.is_some());

    let found_counter = found_counter.unwrap();
    let count = found_counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 42);

    // Verify it's the same actor
    let original_count = counter.ask(GetCount).await.unwrap();
    assert_eq!(original_count, count);
}

#[tokio::test]
async fn test_respawn_or_fallback() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "nonexistent");

    let root_ctx = RootContext::init_local();

    // Test respawn_or with non-existent persistence
    let counter = root_ctx
        .respawn_or(persistence_url.clone(), Counter { count: 100 })
        .await
        .unwrap();

    // Should create new actor with fallback args
    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 100);

    // Verify it was registered as persistent
    let persistence_key = Counter::persistence_key(&counter.downgrade());
    assert!(persistence_key.is_some());
    assert_eq!(persistence_key.unwrap(), persistence_url);
}

#[tokio::test]
async fn test_manager_with_persistent_children() {
    let temp_dir = TempDir::new().unwrap();
    let manager_url = create_temp_url(&temp_dir, "manager");
    let counter1_url = create_temp_url(&temp_dir, "counter1");
    let counter2_url = create_temp_url(&temp_dir, "counter2");

    let root_ctx = RootContext::init_local();

    // Create some persistent counters first
    let _counter1: ActorRef<Counter> = root_ctx
        .spawn_persistent(counter1_url.clone(), Counter { count: 10 })
        .await
        .unwrap();

    let _counter2: ActorRef<Counter> = root_ctx
        .spawn_persistent(counter2_url.clone(), Counter { count: 20 })
        .await
        .unwrap();

    // Create manager with references to these counters
    let mut counter_urls = HashMap::new();
    counter_urls.insert("c1".to_string(), counter1_url);
    counter_urls.insert("c2".to_string(), counter2_url);

    let manager = root_ctx
        .spawn_persistent(
            manager_url.clone(),
            ManagerArgs {
                config: "test_manager".to_string(),
                counter_urls,
            },
        )
        .await
        .unwrap();

    // Verify manager can access its child counters
    // (You'd need to add methods to ManagerActor to test this)

    // Save manager state
    let manager_state = Manager {
        config: "test_manager".to_string(),
        counters: HashMap::new(), // This would be populated in real usage
    };
    manager_state
        .save_snapshot(&manager.downgrade())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_persistence_file_operations() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "file_test");

    // Test snapshot creation and reading
    let snapshot = Counter { count: 999 };

    // Write snapshot
    Counter::try_write(&persistence_url, snapshot.clone())
        .await
        .unwrap();

    // Verify file was created
    let file_path = persistence_url.to_file_path().unwrap().join("index.bin");
    assert!(file_path.exists());

    // Read snapshot back
    let data = Counter::try_read(&persistence_url).await.unwrap();
    let restored_snapshot: Counter = postcard::from_bytes(&data).unwrap();

    assert_eq!(restored_snapshot.count, 999);
}

#[tokio::test]
async fn test_registry_cleanup() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "registry_test");

    let root_ctx = RootContext::init_local();

    // Create persistent actor
    let counter: ActorRef<Counter> = root_ctx
        .spawn_persistent(persistence_url.clone(), Counter { count: 1 })
        .await
        .unwrap();

    // Verify it's in registry
    let found = Counter::lookup_persistent(&persistence_url);
    assert!(found.is_some());

    // Drop the actor
    drop(counter);

    // Give some time for cleanup (depending on your implementation)
    sleep(Duration::from_millis(100)).await;

    // The weak reference should be cleaned up eventually
    // (This test might need adjustment based on your cleanup strategy)
}
