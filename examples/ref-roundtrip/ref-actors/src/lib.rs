use serde::{Deserialize, Serialize};
use theta::prelude::*;
use theta_macros::ActorArgs;

#[cfg(feature = "ts")]
use theta_macros::TsType;

// ── Item actor ────────────────────────────────────────────────────────────────
//
// A simple counter/echo actor used as the "leaf" in all ref round-trip
// scenarios.  Spawned dynamically by the Depot actor.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Ping;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Bump;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct GetCount;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct ItemState {
    pub label: String,
    pub count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ActorArgs)]
pub struct Item {
    pub label: String,
    pub count: u32,
}

#[actor("f1a2b3c4-d5e6-7890-abcd-0123456789ab", ts)]
impl Actor for Item {
    type View = ItemState;

    // T1/T3/T4/T5 validity check — call this through a round-tripped ref.
    const _: () = async |_: Ping| -> String { format!("pong:{}", self.label) };

    const _: () = async |_: Bump| {
        self.count += 1;
    };

    const _: () = async |_: GetCount| -> u32 { self.count };

    fn hash_code(&self) -> u64 {
        self.count as u64
    }
}

impl From<&Item> for ItemState {
    fn from(i: &Item) -> Self {
        Self {
            label: i.label.clone(),
            count: i.count,
        }
    }
}

// ── Depot messages (scenarios) ────────────────────────────────────────────────

/// T1: `ask(SpawnItem)` returns `ActorRef<Item>` — ActorRef as return value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct SpawnItem {
    pub label: String,
}

/// T2: `ask(Relay)` — ActorRef<Item> as a field inside the request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Relay {
    pub target: ActorRef<Item>,
}

/// T3: `ask(GetBundle)` returns `Bundle` — struct with two ActorRef fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct GetBundle;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Bundle {
    pub a: ActorRef<Item>,
    pub b: ActorRef<Item>,
}

/// T4: `ask(GetGroup)` returns `Group` — struct with Vec<ActorRef<Item>>.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct GetGroup;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Group {
    pub members: Vec<ActorRef<Item>>,
}

/// T5: `ask(GetNested)` — ActorRef inside a nested plain struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct GetNested;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Shelf {
    pub item: ActorRef<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct Container {
    pub inner: Shelf,
}

// ── Depot actor ───────────────────────────────────────────────────────────────
//
// Coordinator that manages a set of Item actors.  The native binary pre-seeds
// it with three items before exposing it to the WASM/TS side via bind().

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TsType))]
pub struct DepotState {
    pub count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ActorArgs)]
pub struct Depot {
    /// Live Item refs — skipped in serde so spawnDepot({}) works from TS.
    #[serde(skip)]
    pub items: Vec<ActorRef<Item>>,
}

#[actor("b0c1d2e3-f4a5-6789-bcde-f01234567890", ts)]
impl Actor for Depot {
    type View = DepotState;

    // T1: return value IS an ActorRef.
    const _: () = async |msg: SpawnItem| -> ActorRef<Item> {
        let item_ref = ctx.spawn(Item {
            label: msg.label,
            count: 0,
        });
        self.items.push(item_ref.clone());
        item_ref
    };

    // T2: incoming message HAS an ActorRef field; depot pings it and echoes.
    const _: () = async |msg: Relay| -> String {
        msg.target
            .ask(Ping)
            .await
            .unwrap_or_else(|_| "relay-error".to_string())
    };

    // T3: response is a struct with two ActorRef fields.
    const _: () = async |_: GetBundle| -> Bundle {
        Bundle {
            a: self.items[0].clone(),
            b: self.items[1].clone(),
        }
    };

    // T4: response contains Vec<ActorRef<Item>>.
    const _: () = async |_: GetGroup| -> Group {
        Group {
            members: self.items.clone(),
        }
    };

    // T5: response is a nested struct where the innermost leaf is an ActorRef.
    const _: () = async |_: GetNested| -> Container {
        Container {
            inner: Shelf {
                item: self.items[0].clone(),
            },
        }
    };

    fn hash_code(&self) -> u64 {
        self.items.len() as u64
    }
}

impl From<&Depot> for DepotState {
    fn from(d: &Depot) -> Self {
        Self {
            count: d.items.len() as u32,
        }
    }
}
