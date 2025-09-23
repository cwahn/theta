use serde::{Deserialize, Serialize};
use theta::prelude::*;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Clone, ActorArgs)]
struct SomeActor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoneDisplayResult;

#[derive(Debug, Error, Serialize, Deserialize)]
#[error("a simple error")]
pub struct SimpleError;

#[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
impl Actor for SomeActor {
    // Behaviors will generate single enum Msg for the actor
    const _: () = {
        async |_: NoError| -> u64 { 42 };

        async |_: ErrorResult| -> Result<u64, SimpleError> { Err(SimpleError) };

        async |_: DisplayResult| -> Result<u64, String> {
            Err("a string error occurred".to_string())
        };

        async |_: NoneDisplayResult| -> Result<u64, ()> { Err(()) }
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("trace").init();
    tracing_log::LogTracer::init().ok();

    let ctx = RootContext::init_local();
    let actor = ctx.spawn(SomeActor);

    // tells
    info!("result::Err returned from `tell` will be printed out as tracing::error");
    info!("two lines of error expected");
    let _ = actor.tell(NoError);
    let _ = actor.tell(ErrorResult);
    let _ = actor.tell(DisplayResult);
    let _ = actor.tell(NoneDisplayResult);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    info!("`ask` does not log error, it should be handled by caller");
    let _ = actor.ask(NoError).await?;
    let _ = actor.ask(ErrorResult).await;
    let _ = actor.ask(DisplayResult).await;
    let _ = actor.ask(NoneDisplayResult).await;

    Ok(())
}
