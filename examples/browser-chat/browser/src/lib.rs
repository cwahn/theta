use browser_chat_shared::{ChatRoom, SendMessage};
use iroh::{Endpoint, endpoint::presets};
use theta::prelude::*;
use wasm_bindgen::prelude::*;

static CHAT_REF: std::sync::OnceLock<ActorRef<ChatRoom>> = std::sync::OnceLock::new();
static PUBLIC_KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Create a new chat room on this peer. Returns the public key for others to join.
#[wasm_bindgen]
pub async fn create_room() -> Result<String, JsValue> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await
        .map_err(|e| JsValue::from_str(&format!("endpoint bind failed: {e}")))?;

    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key().to_string();

    let chat = ctx.spawn(ChatRoom::new());
    ctx.bind("chat", chat.clone())
        .map_err(|e| JsValue::from_str(&format!("bind failed: {e}")))?;

    CHAT_REF
        .set(chat)
        .map_err(|_| JsValue::from_str("already initialized"))?;
    PUBLIC_KEY
        .set(public_key.clone())
        .map_err(|_| JsValue::from_str("already initialized"))?;

    Ok(public_key)
}

/// Join an existing chat room hosted by another peer.
#[wasm_bindgen]
pub async fn join_room(host_public_key: String) -> Result<(), JsValue> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await
        .map_err(|e| JsValue::from_str(&format!("endpoint bind failed: {e}")))?;

    let _ctx = RootContext::init(endpoint);

    let url = format!("iroh://chat@{host_public_key}");
    let chat: ActorRef<ChatRoom> = ActorRef::lookup(&url)
        .await
        .map_err(|e| JsValue::from_str(&format!("lookup failed: {e}")))?;

    CHAT_REF
        .set(chat)
        .map_err(|_| JsValue::from_str("already initialized"))?;

    Ok(())
}

#[wasm_bindgen]
pub async fn send_message(author: String, text: String) -> Result<(), JsValue> {
    let chat = CHAT_REF
        .get()
        .ok_or_else(|| JsValue::from_str("not connected"))?;

    chat.tell(SendMessage { author, text })
        .map_err(|e| JsValue::from_str(&format!("send failed: {e}")))?;

    Ok(())
}

#[wasm_bindgen]
pub async fn start_monitoring(callback: js_sys::Function) -> Result<(), JsValue> {
    let chat = CHAT_REF
        .get()
        .ok_or_else(|| JsValue::from_str("not connected"))?;

    let (tx, rx) = theta_flume::unbounded_anonymous();

    chat.monitor(tx)
        .await
        .map_err(|e| JsValue::from_str(&format!("monitor failed: {e}")))?;

    wasm_bindgen_futures::spawn_local(async move {
        while let Some(update) = rx.recv().await {
            if let Update::State(messages) = update {
                let json = match serde_json::to_string(&messages) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&json));
            }
        }
    });

    Ok(())
}
