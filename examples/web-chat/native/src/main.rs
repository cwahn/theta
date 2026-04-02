use axum::{
    Router,
    body::Body,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chat_rooms::{ChatMessage, ChatRoom, GetHistory, SendMessage};
use iroh::{Endpoint, dns::DnsResolver, endpoint::presets};
use rust_embed::RustEmbed;
use theta::prelude::*;
use tracing_subscriber::fmt::time::ChronoLocal;

#[derive(RustEmbed)]
#[folder = "../react-app/dist/"]
struct WebAssets;

fn serve_embedded(path: &str) -> Response {
    match <WebAssets as RustEmbed>::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(file.data.to_vec()))
                .unwrap()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn index() -> Response {
    serve_embedded("index.html")
}

async fn static_file(Path(path): Path<String>) -> Response {
    serve_embedded(&path)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,theta=trace")
        .with_timer(ChronoLocal::new("%H:%M:%S".into()))
        .compact()
        .init();
    tracing_log::LogTracer::init().ok();

    let args: Vec<String> = std::env::args().collect();

    let dns = DnsResolver::with_nameserver("8.8.8.8:53".parse().unwrap());
    let endpoint = Endpoint::builder(presets::N0)
        .dns_resolver(dns)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await?;

    let ctx = RootContext::init(endpoint);
    let my_key = ctx.public_key();

    match args.get(1).map(|s| s.as_str()) {
        Some("create") => {
            let chat = ctx.spawn(ChatRoom::new());
            ctx.bind("chat", chat).expect("bind failed");

            let port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(9090);

            println!("ROOM_CREATED:{my_key}");
            println!("Serving web UI at http://localhost:{port}");
            println!("Join key: {my_key}");

            let app = Router::new()
                .route("/", axum::routing::get(index))
                .route(
                    "/assets/{*path}",
                    axum::routing::get(|Path(path): Path<String>| async move {
                        serve_embedded(&format!("assets/{path}"))
                    }),
                )
                .route("/{*path}", axum::routing::get(static_file));

            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
            axum::serve(listener, app).await?;
        }
        Some("join") => {
            let host_key = args
                .get(2)
                .expect("usage: web-chat-peer join <host-public-key> [name]");
            let name = args.get(3).map(|s| s.as_str()).unwrap_or("NativePeer");

            let url = format!("iroh://chat@{host_key}");
            println!("Joining {url}...");
            let chat: ActorRef<ChatRoom> = ActorRef::lookup(&url).await?;
            println!("JOINED:{my_key}");

            chat.tell(SendMessage {
                author: name.to_string(),
                text: format!("Hello from {name}!"),
            })?;
            println!("SENT:Hello from {name}!");

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let history: Vec<ChatMessage> = chat.ask(GetHistory).await?;
            println!("HISTORY_COUNT:{}", history.len());
            for msg in &history {
                println!("MSG:{}:{}", msg.author, msg.text);
            }

            std::future::pending::<()>().await;
        }
        _ => {
            eprintln!("Usage:");
            eprintln!(
                "  web-chat-peer create [port]    Create room + serve web UI (default port 9090)"
            );
            eprintln!("  web-chat-peer join <key> [name] Join a room as native peer");
            std::process::exit(1);
        }
    }

    Ok(())
}
