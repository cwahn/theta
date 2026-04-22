use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;
#[cfg(feature = "ts")]
use theta_macros::TsType;

const CHAT_ROOM_UUID: uuid::Uuid = uuid::uuid!("e3a1c7b9-4f2d-4e8a-b6d5-9c0f1a2b3d4e");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct ChatMessage {
    pub author: String,
    pub text: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ActorArgs)]
pub struct ChatRoom {
    pub messages: Vec<ChatMessage>,
}

impl ChatRoom {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct SendMessage {
    pub author: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct GetHistory;

#[actor(CHAT_ROOM_UUID, ts)]
impl Actor for ChatRoom {
    type View = Vec<ChatMessage>;

    const _: () = async |msg: SendMessage| {
        let timestamp = web_time::SystemTime::now()
            .duration_since(web_time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.messages.push(ChatMessage {
            author: msg.author,
            text: msg.text,
            timestamp,
        });
    };

    const _: () = async |_: GetHistory| -> Vec<ChatMessage> { self.messages.clone() };

    fn hash_code(&self) -> u64 {
        let mut hasher = ahash::AHasher::default();

        self.messages.len().hash(&mut hasher);

        hasher.finish()
    }
}

impl From<&ChatRoom> for Vec<ChatMessage> {
    fn from(room: &ChatRoom) -> Self {
        room.messages.clone()
    }
}
