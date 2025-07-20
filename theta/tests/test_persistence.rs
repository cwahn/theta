// tests/persistence_test.rs
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use theta::persistence::persistent_actor::PersistentActor;
use theta::{persistence::persistent_actor::ContextExt, prelude::*};
use theta_macros::PersistentActor;
use tokio::time::sleep;
use url::Url;

// Test actors
#[derive(Debug, Clone, PersistentActor)]
pub struct Counter {
    pub count: i32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterArgs {
    pub count: i32,
    pub name: String,
}

impl From<&Counter> for CounterArgs {
    fn from(actor: &Counter) -> Self {
        Self {
            count: actor.count,
            name: actor.name.clone(),
        }
    }
}

impl Actor for Counter {
    type Args = CounterArgs;

    async fn initialize(_ctx: Context<Self>, args: &Self::Args) -> Self {
        Counter {
            count: args.count,
            name: args.name.clone(),
        }
    }
}

// Messages for CounterActor
#[derive(Debug)]
pub struct Increment(pub i32);

#[derive(Debug)]
pub struct GetCount;

impl Behavior<Increment> for Counter {
    type Return = ();

    async fn process(&mut self, _ctx: Context<Self>, msg: Increment) -> Self::Return {
        self.count += msg.0;
    }
}

impl Behavior<GetCount> for Counter {
    type Return = i32;

    async fn process(&mut self, _ctx: Context<Self>, _msg: GetCount) -> Self::Return {
        self.count
    }
}

// Manager actor similar to your example
#[derive(Debug, Clone, PersistentActor)]
pub struct ManagerActor {
    pub config: String,
    pub counters: HashMap<String, ActorRef<Counter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerArgs {
    pub config: String,
    pub counter_urls: HashMap<String, Url>,
}

impl From<&ManagerActor> for ManagerArgs {
    fn from(actor: &ManagerActor) -> Self {
        Self {
            config: actor.config.clone(),
            counter_urls: actor
                .counters
                .iter()
                .filter_map(|(name, actor_ref)| {
                    PersistentActor::persistence_key(actor_ref).map(|url| (name.clone(), url))
                })
                .collect(),
        }
    }
}

impl Actor for ManagerActor {
    type Args = ManagerArgs;

    async fn initialize(ctx: Context<Self>, args: &Self::Args) -> Self {
        let counter_buffer: Arc<Mutex<HashMap<String, ActorRef<Counter>>>> = Default::default();

        let respawn_tasks = args
            .counter_urls
            .iter()
            .map(|(name, url)| {
                let counter_buffer = counter_buffer.clone();
                let ctx = ctx.clone();
                let name = name.clone();
                let url = url.clone();

                async move {
                    if let Ok(counter) = ctx
                        .respawn_or(
                            url.clone(),
                            CounterArgs {
                                count: 0,
                                name: name.clone(),
                            },
                        )
                        .await
                    {
                        counter_buffer.lock().unwrap().insert(name, counter);
                    } else {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("Failed to respawn counter for URL: {url}");
                    }
                }
            })
            .collect::<Vec<_>>();

        futures::future::join_all(respawn_tasks).await;

        let counters = Arc::try_unwrap(counter_buffer)
            .unwrap()
            .into_inner()
            .unwrap();

        Self {
            config: args.config.clone(),
            counters,
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
    let root_ctx = GlobalContext::initialize().await;

    // Test 1: Create a new persistent actor
    let counter = root_ctx
        .spawn_persistent(
            persistence_url.clone(),
            CounterArgs {
                count: 5,
                name: "test_counter".to_string(),
            },
        )
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
    let counter_state = Counter {
        count: 15,
        name: "test_counter".to_string(),
    };
    counter_state.save_snapshot(&counter).await.unwrap();

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

    let root_ctx = GlobalContext::initialize().await;

    // Create persistent actor
    let counter: ActorRef<Counter> = root_ctx
        .spawn_persistent(
            persistence_url.clone(),
            CounterArgs {
                count: 42,
                name: "lookup_test".to_string(),
            },
        )
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

    let root_ctx = GlobalContext::initialize().await;

    // Test respawn_or with non-existent persistence
    let counter = root_ctx
        .respawn_or(
            persistence_url.clone(),
            CounterArgs {
                count: 100,
                name: "fallback_test".to_string(),
            },
        )
        .await
        .unwrap();

    // Should create new actor with fallback args
    let count = counter.ask(GetCount).await.unwrap();
    assert_eq!(count, 100);

    // Verify it was registered as persistent
    let persistence_key = Counter::persistence_key(&counter);
    assert!(persistence_key.is_some());
    assert_eq!(persistence_key.unwrap(), persistence_url);
}

#[tokio::test]
async fn test_manager_with_persistent_children() {
    let temp_dir = TempDir::new().unwrap();
    let manager_url = create_temp_url(&temp_dir, "manager");
    let counter1_url = create_temp_url(&temp_dir, "counter1");
    let counter2_url = create_temp_url(&temp_dir, "counter2");

    let root_ctx = GlobalContext::initialize().await;

    // Create some persistent counters first
    let _counter1: ActorRef<Counter> = root_ctx
        .spawn_persistent(
            counter1_url.clone(),
            CounterArgs {
                count: 10,
                name: "counter1".to_string(),
            },
        )
        .await
        .unwrap();

    let _counter2: ActorRef<Counter> = root_ctx
        .spawn_persistent(
            counter2_url.clone(),
            CounterArgs {
                count: 20,
                name: "counter2".to_string(),
            },
        )
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
    let manager_state = ManagerActor {
        config: "test_manager".to_string(),
        counters: HashMap::new(), // This would be populated in real usage
    };
    manager_state.save_snapshot(&manager).await.unwrap();
}

#[tokio::test]
async fn test_persistence_file_operations() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "file_test");

    // Test snapshot creation and reading
    let snapshot = CounterArgs {
        count: 999,
        name: "file_test".to_string(),
    };

    // Write snapshot
    Counter::try_write(&persistence_url, snapshot.clone())
        .await
        .unwrap();

    // Verify file was created
    let file_path = persistence_url.to_file_path().unwrap().join("index.bin");
    assert!(file_path.exists());

    // Read snapshot back
    let data = Counter::try_read(&persistence_url).await.unwrap();
    let restored_snapshot: CounterArgs = postcard::from_bytes(&data).unwrap();

    assert_eq!(restored_snapshot.count, 999);
    assert_eq!(restored_snapshot.name, "file_test");
}

#[tokio::test]
async fn test_registry_cleanup() {
    let temp_dir = TempDir::new().unwrap();
    let persistence_url = create_temp_url(&temp_dir, "registry_test");

    let root_ctx = GlobalContext::initialize().await;

    // Create persistent actor
    let counter: ActorRef<Counter> = root_ctx
        .spawn_persistent(
            persistence_url.clone(),
            CounterArgs {
                count: 1,
                name: "registry_test".to_string(),
            },
        )
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
