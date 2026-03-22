use browser_chat_shared::ChatRoom;
use iroh::{Endpoint, endpoint::presets};
use theta::prelude::*;
use tracing_subscriber::fmt::time::ChronoLocal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%H:%M:%S".into()))
        .compact()
        .init();

    tracing_log::LogTracer::init().ok();

    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key();

    let chat = ctx.spawn(ChatRoom::new());
    ctx.bind("chat", chat).expect("failed to bind chat actor");

    println!("Chat server running.");
    println!("Public key: {public_key}");
    println!("Give this key to the browser client to connect.");

    std::future::pending::<()>().await;
    Ok(())
}
