// #[cfg(test)]
// mod tests {

//     use std::sync::Arc;
//     use tokio::sync::Notify;
//     use tokio::time::{Duration, timeout};
//     use uuid::Uuid;

//     use crate::prelude::*;

//     #[derive(Debug)]
//     struct Nil;

//     impl Actor for Nil {
//         type Args = ();

//         async fn initialize(_ctx: Context<Self>, _args: &Self::Args) -> Self {
//             Nil
//         }
//     }

//     // Test actor that panics during init for escalation testing
//     #[derive(Debug)]
//     struct PanicOnInit;

//     impl Actor for PanicOnInit {
//         type Args = ();

//         async fn initialize(_ctx: Context<Self>, _args: &Self::Args) -> Self {
//             panic!("Init panic for testing");
//         }
//     }

//     #[derive(Debug)]
//     struct Counter {
//         count: u32,
//     }

//     impl Actor for Counter {
//         type Args = ();

//         async fn initialize(_ctx: Context<Self>, _args: &Self::Args) -> Self {
//             Counter { count: 0 }
//         }
//     }

//     #[derive(Debug)]
//     struct GetCount;

//     impl Behavior<GetCount> for Counter {
//         type Return = u32;

//         async fn process(&mut self, _ctx: Context<Self>, _msg: GetCount) -> Self::Return {
//             self.count
//         }
//     }

//     #[derive(Debug)]
//     struct Increase;

//     impl Behavior<Increase> for Counter {
//         type Return = u32;

//         async fn process(&mut self, _ctx: Context<Self>, _msg: Increase) -> Self::Return {
//             self.count += 1;
//             self.count
//         }
//     }

//     #[derive(Debug)]
//     struct Decrease;

//     impl Behavior<Decrease> for Counter {
//         type Return = u32;

//         async fn process(&mut self, _ctx: Context<Self>, _msg: Decrease) -> Self::Return {
//             self.count -= 1;
//             self.count
//         }
//     }

//     // Test actor with custom supervision behavior
//     #[derive(Debug)]
//     struct Manager {
//         children: Vec<ActorRef<Nil>>,
//         restart_called: Arc<Notify>,
//     }

//     impl Actor for Manager {
//         type Args = Arc<Notify>;

//         async fn initialize(_ctx: Context<Self>, args: &Self::Args) -> Self {
//             Manager {
//                 children: Vec::new(),
//                 restart_called: args.clone(),
//             }
//         }

//         async fn supervise(&mut self, _escalation: Escalation) -> (Signal, Option<Signal>) {
//             self.restart_called.notify_one();
//             (Signal::Restart, None) // Simulate restart signal
//         }
//     }

//     #[derive(Debug)]
//     pub struct CreateChild;

//     impl Behavior<CreateChild> for Manager {
//         type Return = ActorRef<Nil>;

//         async fn process(&mut self, ctx: Context<Self>, _msg: CreateChild) -> Self::Return {
//             let child: ActorRef<Nil> = ctx.spawn(()).await;
//             self.children.push(child.clone());
//             child
//         }
//     }

//     #[derive(Debug)]
//     pub struct RemoveChild(Uuid);

//     impl Behavior<RemoveChild> for Manager {
//         type Return = bool;

//         async fn process(&mut self, _ctx: Context<Self>, msg: RemoveChild) -> Self::Return {
//             if let Some(pos) = self.children.iter().position(|c| c.id() == msg.0) {
//                 self.children.remove(pos);
//                 true
//             } else {
//                 false
//             }
//         }
//     }

//     #[derive(Debug)]
//     pub struct GetChild(Uuid);

//     impl Behavior<GetChild> for Manager {
//         type Return = Option<ActorRef<Nil>>;

//         async fn process(&mut self, _ctx: Context<Self>, msg: GetChild) -> Self::Return {
//             self.children.iter().find(|c| c.id() == msg.0).cloned()
//         }
//     }

//     // Test actor with lifecycle hooks
//     #[derive(Debug)]
//     struct LifecycleNotifier {
//         restart_notify: Arc<Notify>,
//         exit_notify: Arc<Notify>,
//     }

//     impl Actor for LifecycleNotifier {
//         type Args = (Arc<Notify>, Arc<Notify>);

//         async fn initialize(_ctx: Context<Self>, args: &Self::Args) -> Self {
//             LifecycleNotifier {
//                 restart_notify: args.0.clone(),
//                 exit_notify: args.1.clone(),
//             }
//         }

//         async fn on_restart(&mut self) {
//             self.restart_notify.notify_one();
//         }

//         async fn on_exit(&mut self, _exit_code: ExitCode) {
//             self.exit_notify.notify_one();
//         }
//     }

//     // Tests

//     mod actor_tests {
//         use anyhow::Ok;

//         use super::*;

//         #[tokio::test]
//         async fn test_actor_init() {
//             let nil = spawn::<Nil>(()).await;
//             // If we get here without panic, init worked
//             assert!(nil.is_closed() == false);
//         }

//         #[tokio::test]
//         async fn test_actor_spawn_child() {
//             #[derive(Debug)]
//             struct ParentActor;

//             impl Actor for ParentActor {
//                 type Args = Arc<Notify>;

//                 async fn initialize(ctx: Context<Self>, args: &Self::Args) -> Self {
//                     let _child = ctx.spawn::<Nil>(()).await;
//                     args.notify_one();
//                     ParentActor
//                 }
//             }

//             let notify = Arc::new(Notify::new());
//             let _parent = spawn::<ParentActor>(notify.clone()).await;

//             // Wait for child spawn to complete
//             timeout(Duration::from_millis(100), notify.notified())
//                 .await
//                 .expect("Child should be spawned");
//         }

//         #[tokio::test]
//         async fn test_simple_actor() {
//             let counter = spawn::<Counter>(()).await;

//             counter.tell(Increase).unwrap();
//             let final_count = counter.ask(GetCount).await.unwrap();
//             assert_eq!(1, final_count);

//             let new_count = counter.ask(Decrease).await.unwrap();
//             assert_eq!(0, new_count);
//         }

//         #[tokio::test]
//         async fn test_manager_actor() {
//             let restart_notify = Arc::new(Notify::new());
//             let manager = spawn::<Manager>(restart_notify.clone()).await;

//             // Test spawning a child
//             let child = manager.ask(CreateChild).await.unwrap();
//             assert!(child.is_closed() == false);

//             // Test getting the child
//             let found_child = manager.ask(GetChild(child.id())).await.unwrap();
//             assert!(found_child.is_some());
//             assert_eq!(found_child.unwrap().id(), child.id());

//             // Test removing the child
//             let removed = manager.ask(RemoveChild(child.id())).await.unwrap();
//             assert!(removed);
//         }
//     }

//     mod context_tests {

//         use super::*;

//         #[tokio::test]
//         async fn test_context_spawn() {
//             #[derive(Debug)]
//             struct SpawnerActor {
//                 children: Vec<ActorRef<Nil>>,
//                 child_spawned: Arc<Notify>,
//             }

//             impl Actor for SpawnerActor {
//                 type Args = Arc<Notify>;

//                 async fn initialize(ctx: Context<Self>, args: &Self::Args) -> Self {
//                     let child = ctx.spawn::<Nil>(()).await;

//                     args.notify_one();
//                     SpawnerActor {
//                         children: vec![child],
//                         child_spawned: args.clone(),
//                     }
//                 }
//             }

//             let notify = Arc::new(Notify::new());
//             let _spawner = spawn::<SpawnerActor>(notify.clone()).await;

//             timeout(Duration::from_millis(100), notify.notified())
//                 .await
//                 .expect("Context spawn should work");
//         }

//         #[tokio::test]
//         async fn test_super_context_spawn() {
//             let restart_notify = Arc::new(Notify::new());
//             let supervisor = spawn::<Manager>(restart_notify.clone()).await;

//             // Test that SuperContext can spawn (tested indirectly through supervision)
//             assert!(supervisor.is_closed() == false);
//         }
//     }

//     mod system_tests {

//         use super::*;

//         #[tokio::test]
//         async fn test_global_spawn() {
//             let actor = spawn::<Nil>(()).await;
//             assert!(actor.is_closed() == false);
//         }

//         #[tokio::test]
//         async fn test_bind_and_lookup() {
//             let actor = spawn::<Nil>(()).await;
//             let key = "test_actor";

//             bind(key, actor.clone());
//             let found: Option<ActorRef<Nil>> = lookup(key);

//             assert!(found.is_some());

//             #[derive(Debug)]
//             struct OtherActor;
//             impl Actor for OtherActor {
//                 type Args = ();
//                 async fn initialize(_ctx: Context<Self>, _args: &Self::Args) -> Self {
//                     OtherActor
//                 }
//             }

//             let wrong_type: Option<ActorRef<OtherActor>> = lookup(key);
//             assert!(wrong_type.is_none());
//         }

//         #[tokio::test]
//         async fn test_bind_lookup_free() {
//             let actor = spawn::<Nil>(()).await;
//             let key = "free_test";

//             bind(key, actor);
//             assert!(lookup::<Nil>(key).is_some());

//             assert!(free(key));
//             assert!(lookup::<Nil>(key).is_none());

//             // Free non-existent key returns false
//             assert!(!free("non_existent"));
//         }

//         #[tokio::test]
//         async fn test_multiple_bindings() {
//             let actor1 = spawn::<Nil>(()).await;
//             let actor2 = spawn::<Nil>(()).await;

//             bind("actor1", actor1);
//             bind("actor2", actor2);

//             assert!(lookup::<Nil>("actor1").is_some());
//             assert!(lookup::<Nil>("actor2").is_some());
//             assert!(lookup::<Nil>("actor3").is_none());
//         }

//         #[tokio::test]
//         async fn test_binding_overwrite() {
//             let actor1 = spawn::<Nil>(()).await;
//             let actor2 = spawn::<Nil>(()).await;
//             let key = "overwrite_test";

//             bind(key, actor1);
//             bind(key, actor2); // Should overwrite

//             let found: Option<ActorRef<Nil>> = lookup(key);
//             assert!(found.is_some());
//         }
//     }

//     mod supervision_tests {
//         use super::*;

//         #[tokio::test]
//         async fn test_supervision_escalation() {
//             let restart_notify = Arc::new(Notify::new());
//             let supervisor = spawn::<Manager>(restart_notify.clone()).await;

//             // This test would require triggering an actual escalation
//             // For minimal testing, we just verify the supervisor exists
//             assert!(supervisor.is_closed() == false);
//         }
//     }

//     mod lifecycle_tests {
//         use super::*;

//         #[tokio::test]
//         async fn test_lifecycle_hooks_exist() {
//             let restart_notify = Arc::new(Notify::new());
//             let exit_notify = Arc::new(Notify::new());

//             let actor = spawn::<LifecycleNotifier>((restart_notify, exit_notify)).await;

//             // For minimal testing, just verify the actor was created
//             assert!(actor.is_closed() == false);
//         }
//     }
// }
