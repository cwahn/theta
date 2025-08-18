use std::{fmt::Debug, future::Future, panic::UnwindSafe};

use crate::{
    context::Context,
    message::{Continuation, Escalation, Signal},
};

#[cfg(feature = "remote")]
use {
    crate::remote::{base::ActorTypeId, serde::FromTaggedBytes},
    serde::{Deserialize, Serialize},
};

/// Unique identifier for each actor instance.
pub type ActorId = uuid::Uuid;

// ! Currently, the restart sementic should be considered ill-designed.
// todo Redesign resilience system.

/// Trait for actor initialization arguments.
///
/// This trait defines how actors are initialized from their arguments.
/// Typically derived using `#[derive(ActorArgs)]`.
///
/// # Example
///
/// ```
/// use theta::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor {
///     name: String,
///     value: i32,
/// }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Increment(i32);
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {
///     const _: () = {
///         async |Increment(amount): Increment| {
///             self.value += amount;
///         };
///     };
/// }
/// ```
pub trait ActorArgs: Clone + Send + UnwindSafe + 'static {
    type Actor: Actor;

    /// Initialize an actor from the given arguments.
    ///
    /// This method is called when spawning a new actor and should return
    /// the initialized actor state. The method is panic-safe - any panics
    /// will be caught and escalated to the supervisor.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The actor's context for communication and spawning children
    /// * `args` - The initialization arguments
    fn initialize(
        ctx: Context<Self::Actor>,
        args: &Self,
    ) -> impl Future<Output = Self::Actor> + Send + UnwindSafe;
}

/// Core trait that defines actor behavior.
///
/// This trait must be implemented by all actor types. It defines the message type,
/// state reporting type, and behavior methods for message processing and supervision.
///
/// # Usage with `#[actor]` Macro
///
/// Actors are typically implemented using the `#[actor]` attribute macro which
/// generates the necessary boilerplate. The macro provides significant conveniences
/// and automatic implementations.
///
/// ## Behavior Specification Syntax
///
/// The `#[actor]` macro allows you to specify actor behavior using a special syntax
/// within the `const _: () = {}` block. Inside this block:
///
/// - `&mut self` - Reference to the actor instance (automatically provided)
/// - `ctx` - The actor's context for communication and spawning (automatically provided)
/// - Message handlers use async closure syntax: `async |MessageType(data): MessageType| { ... }`
///
/// ## Basic Example
///
/// ```ignore
/// use theta::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct Counter { value: i64 }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Increment(i64);
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct GetValue;
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for Counter {
///     const _: () = {
///         // &mut self and ctx are automatically available
///         async |Increment(amount): Increment| {
///             self.value += amount;  // &mut self access
///             println!("Incremented to {}", self.value);
///         };
///         
///         async |GetValue: GetValue| {
///             // Use ctx for communication
///             let response = self.value;
///             // ctx.reply(response).await; // Example context usage
///         };
///     };
/// }
/// ```
///
/// ## Default Implementations Provided by Macro
///
/// The `#[actor]` macro automatically provides:
/// - `type StateReport = Nil;` (unless you specify a custom type)
/// - Empty message handler block if no handlers are specified
/// - Message enum generation and dispatch logic
/// - Remote communication support when the `remote` feature is enabled
///
/// ## Advanced Usage
///
/// You can customize state reporting and add supervision logic:
///
/// ```ignore
/// use theta::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct DatabaseActor {
///     connection_pool: String,
///     active_connections: u32,
/// }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Query(String);
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct DbStats {
///     active_connections: u32,
///     uptime: u64,
/// }
///
/// #[actor("87654321-4321-8765-dcba-987654321fed")]
/// impl Actor for DatabaseActor {
///     type StateReport = DbStats;
///     
///     const _: () = {
///         async |Query(sql): Query| {
///             // &mut self is available for state modification
///             self.active_connections += 1;
///             
///             // ctx is available for communication
///             println!("Executing query: {sql}");
///             
///             // Simulate query processing
///             tokio::time::sleep(std::time::Duration::from_millis(10)).await;
///             
///             self.active_connections -= 1;
///         };
///     };
/// }
/// ```
///
/// ## What's Automatically Available in Message Handlers
///
/// Within each message handler closure, you have automatic access to:
/// - `&mut self` - Mutable reference to the actor instance for state modification
/// - `ctx: Context<Self>` - Actor context for:
///   - Spawning child actors (`ctx.spawn`, `ctx.spawn_auto`)
///   - Sending messages to other actors
///   - Accessing actor metadata (`ctx.id()`, `ctx.parent()`)
///   - Lifecycle management (`ctx.stop()`)
///
/// These are provided transparently by the macro, so you can use them freely
/// without explicit parameter declarations.
pub trait Actor: Sized + Debug + Send + UnwindSafe + 'static {
    /// The message type this actor can receive.
    ///
    /// For local-only actors, this can be any `Send` type.
    /// For remote-capable actors (with `remote` feature), messages must also
    /// implement `Serialize`, `Deserialize`, and `FromTaggedBytes`.
    #[cfg(not(feature = "remote"))]
    type Msg: Send;
    #[cfg(feature = "remote")]
    type Msg: Send + Serialize + for<'de> Deserialize<'de> + FromTaggedBytes;

    /// Type used for reporting actor state to monitors.
    ///
    /// This type represents a snapshot of the actor's state that can be
    /// sent to observers. It must implement `From<&Self>` to convert from
    /// the actor's current state.
    #[cfg(not(feature = "remote"))]
    type StateReport: Send + UnwindSafe + Clone + for<'a> From<&'a Self> + 'static;

    #[cfg(feature = "remote")]
    type StateReport: Send
        + UnwindSafe
        + Clone
        + for<'a> From<&'a Self>
        + Serialize
        + for<'de> Deserialize<'de>;

    /// Process incoming messages.
    ///
    /// This method is called for each message received by the actor.
    /// It is panic-safe - panics will be caught and escalated to the supervisor.
    ///
    /// **Note:** This method is typically generated by the `#[actor]` macro
    /// and should not be implemented manually.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The actor's context for communication
    /// * `msg` - The incoming message to process
    /// * `k` - Continuation for handling responses
    #[allow(unused_variables)]
    fn process_msg(
        &mut self,
        ctx: Context<Self>,
        msg: Self::Msg,
        k: Continuation,
    ) -> impl Future<Output = ()> + Send;

    /// Handle escalations from child actors.
    ///
    /// This method is called when a child actor fails and needs supervision.
    /// The default implementation restarts the child actor.
    ///
    /// # Arguments
    ///
    /// * `escalation` - Information about the child actor failure
    ///
    /// # Returns
    ///
    /// A tuple of signals: (signal_to_child, signal_to_parent)
    #[allow(unused_variables)]
    fn supervise(
        &mut self,
        escalation: Escalation,
    ) -> impl Future<Output = (Signal, Option<Signal>)> + Send {
        __default_supervise(self, escalation)
    }

    /// Called when the actor is restarted.
    ///
    /// This lifecycle hook is invoked before re-initialization during a restart.
    /// The default implementation does nothing.
    ///
    /// **Note:** This method is panic-safe, but panics will be logged rather than escalated.
    /// State corruption is possible if this method panics.
    #[allow(unused_variables)]
    fn on_restart(&mut self) -> impl Future<Output = ()> + Send {
        __default_on_restart(self)
    }

    /// Called when the actor is terminating or being dropped.
    ///
    /// This lifecycle hook allows for cleanup operations before the actor shuts down.
    /// The default implementation does nothing.
    ///
    /// **Note:** This method is panic-safe, but panics will be logged rather than escalated.
    /// Since the message loop has stopped, any messages sent to `self` will be lost.
    ///
    /// # Arguments
    ///
    /// * `exit_code` - The reason for termination
    #[allow(unused_variables)]
    fn on_exit(&mut self, exit_code: ExitCode) -> impl Future<Output = ()> + Send {
        __default_on_exit(self, exit_code)
    }

    /// Generate a hash code for this actor instance.
    ///
    /// This method can be used for custom partitioning or sharding logic.
    /// The default implementation returns a constant value.
    #[allow(unused_variables)]
    fn hash_code(&self) -> u64 {
        42 // no-op by default
    }

    /// Generate a state report for monitoring.
    ///
    /// This method creates a snapshot of the actor's current state for observers.
    /// The default implementation uses the `From<&Self>` conversion.
    #[allow(unused_variables)]
    fn state_report(&self) -> Self::StateReport {
        self.into() // no-op by default
    }

    /// Internal implementation ID for remote actors.
    ///
    /// **Note:** This should not be implemented manually - it's generated by the `#[actor]` macro.
    #[cfg(feature = "remote")]
    const IMPL_ID: ActorTypeId;

    // #[cfg(feature = "remote")]
}

/// Reason for actor termination.
#[derive(Debug, Clone)]
pub enum ExitCode {
    /// Actor was dropped normally
    Dropped,
    /// Actor was explicitly terminated
    Terminated,
}

// Delegated default implementation in order to decouple it from macro expansion

pub async fn __default_supervise<A: Actor>(
    _actor: &mut A,
    _escalation: Escalation,
) -> (Signal, Option<Signal>) {
    (Signal::Terminate, None)
}

pub async fn __default_on_restart<A: Actor>(_actor: &mut A) {}

pub async fn __default_on_exit<A: Actor>(_actor: &mut A, _exit_code: ExitCode) {}
