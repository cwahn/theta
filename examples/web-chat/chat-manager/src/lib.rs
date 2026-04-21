use std::collections::HashMap;

use chat_rooms::ChatRoom;
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;
#[cfg(feature = "ts")]
use theta_macros::TsType;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct RoomInfo {
    pub name: String,
    pub room: ActorRef<ChatRoom>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct CreateRoom {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct ListRooms;

#[derive(Debug, Clone, Default, Serialize, Deserialize, ActorArgs)]
pub struct ChatManager {
    rooms: HashMap<String, RoomInfo>,
}

#[actor("a1b2c3d4-e5f6-7890-abcd-ef1234567890", ts)]
impl Actor for ChatManager {
    type View = Vec<RoomInfo>;

    const _: () = async |msg: CreateRoom| -> RoomInfo {
        let room = ChatRoom::new();
        let room_ref = ctx.spawn(room);
        let room_id = room_ref.id().to_string();
        let info = RoomInfo {
            name: msg.name.clone(),
            room: room_ref,
        };

        self.rooms.insert(room_id, info.clone());

        info
    };

    const _: () = async |_: ListRooms| -> Vec<RoomInfo> { self.rooms.values().cloned().collect() };

    fn hash_code(&self) -> u64 {
        self.rooms.len() as u64
    }
}

impl From<&ChatManager> for Vec<RoomInfo> {
    fn from(m: &ChatManager) -> Self {
        m.rooms.values().cloned().collect()
    }
}
