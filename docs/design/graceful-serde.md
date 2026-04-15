# Graceful Serde — PEER-Dispatched ActorRef Serialization

*Supersedes: `encode-decode.md` (Encode/Decode traits abandoned — UX cost too high)*

## Problem

`ActorRef<A>` implements `Serialize`/`Deserialize` via `ActorRefDto`, which requires
a `PEER` task-local scope.  Calling `serde` outside that scope (e.g., at the JS/WASM
boundary) **panics at runtime**.

The Encode/Decode approach solved this by removing serde from `ActorRef` entirely,
but introduced unacceptable UX ceremony: new traits, `#[derive(Data)]` on user types
containing `ActorRef`, and `impl_codec_serde!` in user code.

## Solution

Use `PEER.try_get()` as a mode discriminator inside the existing serde impls:

| `PEER.try_get()` | Context | Serialize | Deserialize |
|---|---|---|---|
| `Some(peer)` | Network (remote handler) | `ActorRefDto` (existing logic, unchanged) | Full resolution via `ActorRefDto` (existing logic, unchanged) |
| `None` | Outside network scope | `self.id().serialize(serializer)` | Local lookup by `ActorId` |

### Why this works

- **Network handlers** always run inside `PEER.scope()` — the `Some` arm fires, producing identical wire bytes.
- **JS/WASM boundary** (future ts-integration) runs without `PEER.scope()` — the `None` arm fires, enabling platform-specific fallback.
- On **wasm32 + ts**, the `None` arm will be replaced by `serde_wasm_bindgen::preserve` (opaque JsValue passthrough). This is the ts-integration phase, not implemented here.

### Hot-path cost

Zero. One well-predicted branch (`Some` in network path, `None` at JS boundary) replaces one always-succeeds `.expect()`.

### Wire compatibility

100%. The `Some` arm produces identical `ActorRefDto` — no format change.

## Implementation

### 1. `try_get()` on `compat_task_local!`

New method on both platform variants. Returns `Option<$ty>` instead of panicking.

**tokio** (native):
```rust
pub fn try_get() -> Option<$ty> {
    $name.try_with(|v| v.clone()).ok()
}
```

**wasm_browser**:
```rust
pub fn try_get(&self) -> Option<$ty> {
    Self::INNER.with(|cell| cell.borrow().clone())
}
```

### 2. Updated `impl Serialize for ActorRef<A>`

```rust
impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match PEER.try_get() {
            Some(_) => ActorRefDto::from(self).serialize(serializer),
            None => self.id().serialize(serializer),
        }
    }
}
```

### 3. Updated `impl Deserialize for ActorRef<A>`

```rust
impl<'de, A: Actor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match PEER.try_get() {
            Some(_) => ActorRefDto::deserialize(deserializer)?
                .try_into()
                .map_err(|e| serde::de::Error::custom(format!(
                    "failed to construct ActorRef from ActorRefDto: {e}"
                ))),
            None => {
                let actor_id = ActorId::deserialize(deserializer)?;
                ActorRef::<A>::lookup_local_impl(actor_id.as_bytes())
                    .map_err(|e| serde::de::Error::custom(format!(
                        "failed to lookup local ActorRef: {e}"
                    )))
            }
        }
    }
}
```

## Decisions

| ID | Decision |
|----|----------|
| G1 | `PEER.try_get()` is the sole mode discriminator — no new traits, no new derives |
| G2 | `None` serialize fallback: `ActorId` (sufficient for local-only contexts) |
| G3 | `None` deserialize fallback: local lookup by `ActorId` (error if not found) |
| G4 | Future wasm32+ts: `None` arm replaced by preserve-based path (ts-integration phase) |
| G5 | Existing `From<&ActorRef<A>> for ActorRefDto` and `TryFrom<ActorRefDto>` unchanged — only called when PEER is present |
| G6 | All existing network tests unaffected — they run inside `PEER.scope()` |
