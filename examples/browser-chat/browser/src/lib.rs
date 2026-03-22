use browser_chat_shared::{ChatMessage, ChatRoom, SendMessage};
use iroh::{Endpoint, endpoint::presets};
use theta::prelude::*;
use wasm_bindgen::prelude::*;

static CHAT_REF: std::sync::OnceLock<ActorRef<ChatRoom>> = std::sync::OnceLock::new();

#[wasm_bindgen]
pub async fn connect(server_public_key: String) -> Result<(), JsValue> {
    web_sys::console::log_1(&format!("[theta] connecting to server: {server_public_key}").into());

    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await
        .map_err(|e| JsValue::from_str(&format!("endpoint bind failed: {e}")))?;

    let _ctx = RootContext::init(endpoint);

    let url = format!("iroh://chat@{server_public_key}");
    web_sys::console::log_1(&format!("[theta] looking up: {url}").into());

    let chat: ActorRef<ChatRoom> = ActorRef::lookup(&url)
        .await
        .map_err(|e| JsValue::from_str(&format!("lookup failed: {e}")))?;

    CHAT_REF
        .set(chat)
        .map_err(|_| JsValue::from_str("already connected"))?;

    web_sys::console::log_1(&"[theta] connected to chat room".into());
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
pub async fn get_history() -> Result<JsValue, JsValue> {
    let chat = CHAT_REF
        .get()
        .ok_or_else(|| JsValue::from_str("not connected"))?;

    let history: Vec<ChatMessage> = chat
        .ask(browser_chat_shared::GetHistory)
        .await
        .map_err(|e| JsValue::from_str(&format!("ask failed: {e}")))?;

    let json = serde_json::to_string(&history)
        .map_err(|e| JsValue::from_str(&format!("json failed: {e}")))?;

    Ok(JsValue::from_str(&json))
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

    web_sys::console::log_1(&"[theta] monitoring started".into());
    Ok(())
}
