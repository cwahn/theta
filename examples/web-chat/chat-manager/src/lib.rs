use std::collections::HashMap;

use chat_rooms::ChatRoom;
use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;

#[cfg(feature = "ts")]
use theta_macros::TsType;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct CreateRoom {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct ResolveRoom {
    pub room_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ActorArgs)]
pub struct ChatManager {
    #[serde(skip)]
    rooms: HashMap<String, ActorRef<ChatRoom>>,
}

#[actor("a1b2c3d4-e5f6-7890-abcd-ef1234567890", ts)]
impl Actor for ChatManager {
    const _: () = async |_msg: CreateRoom| -> String {
        let room = ChatRoom::new();
        let room_ref = ctx.spawn(room);
        let room_id = room_ref.id().to_string();
        self.rooms.insert(room_id.clone(), room_ref);
        room_id
    };

    const _: () = async |msg: ResolveRoom| -> ActorRef<ChatRoom> {
        self.rooms
            .get(&msg.room_id)
            .cloned()
            .expect("Room not found")
    };
}
