# TypeScript Integration — Preserve-Based ActorRef Isomorphism

## Problem

The current TS/WASM bridge in `theta-macros/src/ts.rs` special-cases `ActorRef` at
the top level of return types (`extract_actor_ref_inner`) but fails for any other
position.  Any nesting — `Vec<ActorRef<X>>`, `Option<ActorRef<X>>`,
`Struct { ref: ActorRef<X> }`, or deeper — falls through to `serde_wasm_bindgen`
which calls `ActorRef::serialize`.  That impl only works inside a `PEER` task-local
scope (it produces `ActorRefDto` for the network wire) and **panics at runtime** on
WASM.

Concrete broken scenarios today:

| Pattern | Example | Failure mode |
|---------|---------|-------------|
| `Vec<ActorRef<X>>` return | `GetWorkers → Vec<ActorRef<Worker>>` | `to_value` → panic (no PEER) |
| `Option<ActorRef<X>>` return | `GetCounter → Option<ActorRef<Counter>>` | `to_value` → panic |
| Message field with `ActorRef` | `Ping { target: ActorRef<X> }` | `from_value` → deser error |
| View containing `ActorRef` | `Manager { worker: ActorRef<Counter> }` | `to_value` → panic |
| Spawn args with `ActorRef` | `Manager { worker: ActorRef<Counter> }` | `from_value` → deser error |
| `#[serde(skip)]` workaround | `ChatManager.rooms` | Data silently lost |

## Prerequisite

The **graceful serde migration** (`docs/design/graceful-serde.md`) must land first.
It makes `ActorRef`'s serde impls safe outside `PEER` scope by dispatching on
`PEER.try_get()` — `Some` → network path (unchanged), `None` → platform fallback.
This frees the `None` arm for wasm32+ts preserve-based serialization.

## Solution

Use `serde_wasm_bindgen::preserve` to pass `ActorRef` through serde as opaque
`JsValue` handles — the `XRef` wasm_bindgen wrapper class instances.

### Core idea

On `wasm32` target, `ActorRef<A>` implements `Serialize` and `Deserialize`
using `serde_wasm_bindgen::preserve`:

- **Serialize**: wrap `ActorRef<A>` → `<A as TsActor>::WasmRef` → `JsValue` →
  `preserve::serialize` passes it through.
- **Deserialize**: `preserve::deserialize` yields the raw `JsValue` → `dyn_into`
  recovers the `WasmRef` → extract inner `ActorRef<A>`.

Because `preserve` is transparent to the serde data model, this works at **any
depth** in any container or struct — `Vec`, `Option`, `HashMap`, nested structs —
without any special-case pattern matching or additional derives on user types.

### Type isomorphism

For every Rust type `T`, the JS-side representation `Js(T)` is:

| `T` | `Js(T)` |
|-----|---------|
| `String` | `string` |
| `bool` | `boolean` |
| `u8`..`f64` | `number` |
| `Vec<T>` | `Js(T)[]` |
| `Option<T>` | `Js(T) \| null` |
| `HashMap<K,V>` | `Map<Js(K), Js(V)>` |
| `struct { a: T1, b: T2 }` | `{ a: Js(T1), b: Js(T2) }` |
| `enum (externally tagged)` | `{ Variant: Js(Fields) }` |
| **`ActorRef<X>`** | **`XRef` (opaque wasm_bindgen class)** |

This mapping is applied recursively by serde + preserve.  No case-by-case
code generation.

## Design

### 1. `TsActor` and `TsActorRef` traits (theta-ts)

```rust
/// Implemented by #[actor(ts)] on the actor struct.
pub trait TsActor: Actor {
    type WasmRef: TsActorRef<Self>;
}

/// Implemented by #[actor(ts)] on the generated XRef struct.
pub trait TsActorRef<A: TsActor>: Sized + Into<JsValue> + JsCast {
    fn from_ref(actor_ref: ActorRef<A>) -> Self;
    fn inner_ref(&self) -> ActorRef<A>;
}
```

`JsCast` bound enables `dyn_into` during deserialization.
`Into<JsValue>` enables preserve during serialization.
`inner_ref` is **new** — extracts the inner `ActorRef` from the wrapper.

### 2. `ActorRef` serde impls (wasm32 only)

After the graceful serde migration, `ActorRef`'s `Serialize`/`Deserialize` impls
dispatch on `PEER.try_get()`.  On wasm32 + ts, the `None` arm (no PEER scope)
uses `serde_wasm_bindgen::preserve` to pass `ActorRef` through as opaque `JsValue`:

```rust
// Inside impl Serialize for ActorRef<A>, the None arm becomes:
#[cfg(all(feature = "ts", target_arch = "wasm32"))]
None => {
    let wasm_ref = <A as TsActor>::WasmRef::from_ref(self.clone());
    let js_val: JsValue = wasm_ref.into();
    serde_wasm_bindgen::preserve::serialize(&js_val, serializer)
}

// Inside impl Deserialize for ActorRef<A>, the None arm becomes:
#[cfg(all(feature = "ts", target_arch = "wasm32"))]
None => {
    let js_val: JsValue = serde_wasm_bindgen::preserve::deserialize(deserializer)?;
    let wasm_ref: <A as TsActor>::WasmRef = js_val
        .dyn_into()
        .map_err(|_| serde::de::Error::custom("expected actor ref wrapper"))?;
    Ok(wasm_ref.inner_ref())
}
```

On non-wasm32 or without ts feature, the `None` arm falls back to `ActorId`
serialization (from the graceful serde base implementation).
The `Some` arm (network path) is always unchanged.

### 3. Simplified macro codegen (theta-macros/src/ts.rs)

The entire `extract_actor_ref_inner` special-case and per-variant ask dispatch
is removed.  All paths become uniform:

```rust
// tell — uniform for ALL message types
pub fn tell(&self, msg: JsValue) -> Result<(), JsError> {
    let ts_msg: #ts_msg_enum_ident = serde_wasm_bindgen::from_value(msg)?;
    self.inner.send(ts_msg.into(), Continuation::Nil)?;
    Ok(())
}

// ask — uniform for ALL return types (no ActorRef special case)
pub async fn ask(&self, msg: JsValue) -> Result<JsValue, JsError> {
    let ts_msg: #ts_msg_enum_ident = serde_wasm_bindgen::from_value(msg)?;
    match ts_msg {
        // Every variant: send → await → to_value
        #ts_msg_enum_ident::SomeMsg(inner) => {
            let rust_msg = #rust_enum_ident::__SomeMsg(inner);
            let (tx, rx) = futures::channel::oneshot::channel();
            self.inner.send(rust_msg, Continuation::Reply(tx))?;
            let any = rx.await?;
            let result = *any.downcast::<ReturnType>()?;
            serde_wasm_bindgen::to_value(&result)  // ActorRef fields auto-preserved
        }
    }
}

// prep — uniform
pub async fn prep(&self) -> Result<JsValue, JsError> {
    // ... monitor setup ...
    serde_wasm_bindgen::to_value(&view)  // ActorRef fields in View auto-preserved
}

// initStream — uniform
pub async fn init_stream(&self, callback: js_sys::Function) -> Result<(), JsError> {
    // ... on each update ...
    let js_val = serde_wasm_bindgen::to_value(&view)?;  // auto-preserved
    callback.call1(&JsValue::NULL, &js_val)?;
}

// spawn — uniform
pub fn spawn(args: JsValue) -> Result<XRef, JsError> {
    let actor_args: Actor = serde_wasm_bindgen::from_value(args)?;  // ActorRef fields auto-extracted
    let actor_ref = root_ctx().spawn(actor_args);
    Ok(XRef::from_ref(actor_ref))
}
```

### 4. Generated XRef class

The `#[actor(ts)]` macro generates (all behind `cfg(all(feature = "ts", target_arch = "wasm32"))`):

```rust
#[wasm_bindgen]
pub struct ChatRoomRef {
    inner: ActorRef<ChatRoom>,
}

#[wasm_bindgen]
impl ChatRoomRef {
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String { self.inner.id().to_string() }

    pub fn tell(&self, msg: JsValue) -> Result<(), JsError> { /* uniform */ }
    pub async fn ask(&self, msg: JsValue) -> Result<JsValue, JsError> { /* uniform */ }
    pub async fn prep(&self) -> Result<JsValue, JsError> { /* uniform */ }
    pub async fn init_stream(&self, callback: js_sys::Function) -> Result<(), JsError> { /* uniform */ }
}

impl TsActorRef<ChatRoom> for ChatRoomRef {
    fn from_ref(actor_ref: ActorRef<ChatRoom>) -> Self { Self { inner: actor_ref } }
    fn inner_ref(&self) -> ActorRef<ChatRoom> { self.inner.clone() }
}

impl TsActor for ChatRoom {
    type WasmRef = ChatRoomRef;
}
```

### 5. Free functions

```rust
#[wasm_bindgen]
pub fn spawnChatRoom(args: JsValue) -> Result<ChatRoomRef, JsError> {
    let actor: ChatRoom = serde_wasm_bindgen::from_value(args)?;
    Ok(ChatRoomRef::from_ref(root_ctx().spawn(actor)))
}

#[wasm_bindgen]
pub fn lookupChatRoomLocal(name: &str) -> Result<ChatRoomRef, JsError> {
    Ok(ChatRoomRef::from_ref(ActorRef::<ChatRoom>::lookup_local(name)?))
}

#[wasm_bindgen]
pub fn bindChatRoom(name: &str, handle: &ChatRoomRef) -> Result<(), JsError> {
    root_ctx().bind(name, handle.inner.clone())?;
    Ok(())
}
```

### 6. TypeScript type declarations

The `typescript_custom_section` emits types matching the serde externally-tagged
convention.  `ActorRef<X>` positions map to `XRef`.

```typescript
// Auto-generated in wasm-pack output

export type ChatRoomMsg = { SendMessage: SendMessage } | { GetHistory: GetHistory };

export interface ChatRoomReturns {
  SendMessage: void;
  GetHistory: ChatMessage[];
}

export type ChatRoomView = ChatMessage[];
```

Message types like `SendMessage`, `ChatMessage` etc. are NOT generated by theta —
they follow the serde externally-tagged JSON shape and the user defines them via
`#[derive(TsType)]` only if they want explicit TS interfaces.  Otherwise, the
plain-object shape is implicit from usage.

### 7. Dead code cleanup

The following in `theta-ts/ts/theta-ts/` is unused and should be removed:

| File | Status |
|------|--------|
| `src/core/ref.ts` (generic `ActorRef<A>`) | Dead — replace with thin re-export utilities if needed |
| `src/core/descriptor.ts` (`ActorDescriptor`) | Dead — the generated `typescript_custom_section` replaces this |
| `src/core/stream.ts` (`CachedStream`) | Dead — stream is handled by the generated `initStream` |
| `src/react/hooks.ts` | Useful pattern — but needs rewrite to work with generated `XRef` classes directly |

React hooks should be rewritten to accept the generated `XRef` type directly
(not a generic `ActorRef<ActorDescriptor>`).

## Performance

`serde_wasm_bindgen` does NOT serialize to bytes.  It directly constructs JS
objects via `Reflect` API — the same operations as hand-written conversion code.
`preserve` adds zero overhead: it passes the `JsValue` pointer through.
`dyn_into` is a single prototype chain check.  `inner_ref()` is an `Arc::clone`.

## Migration

### Removed from theta-macros/src/ts.rs
- `extract_actor_ref_inner()` function
- Per-variant ActorRef special-case in ask arms
- `serde_wasm_bindgen::to_value` calls that serialize ActorRef returns specially
- The `is_unit_type` / `is_void` dispatch for ask (replaced with uniform to_value)

### Updated in theta/src/remote/serde.rs
- `impl Serialize for ActorRef<A>` gains PEER.try_get() dispatch (graceful serde)
- `impl Deserialize for ActorRef<A>` gains PEER.try_get() dispatch (graceful serde)
- The `None` arm is the extension point for wasm32+ts preserve-based impls

### Added to theta-ts/src/lib.rs
- `inner_ref()` method on `TsActorRef` trait
- `JsCast` + `Into<JsValue>` supertraits on `TsActorRef`

### Added to theta (behind cfg)
- `impl Serialize for ActorRef<A>` (wasm32 + ts, preserve-based)
- `impl Deserialize for ActorRef<A>` (wasm32 + ts, preserve-based)

### examples/web-chat/chat-manager
- Remove `#[serde(skip)]` on `rooms: HashMap<String, ActorRef<ChatRoom>>`
  (ActorRef now serializes correctly at the JS boundary via preserve)

## Decisions

| ID | Decision |
|----|----------|
| T1 | Use `serde_wasm_bindgen::preserve` for ActorRef at the JS boundary |
| T2 | ActorRef's serde impls are exclusively for JS interop (wasm32 + ts) |
| T3 | No new trait (ToJs/FromJs) needed — serde + preserve is sufficient |
| T4 | No additional derives needed on user message/view types |
| T5 | `#[derive(TsType)]` remains optional — for explicit TS interface generation only |
| T6 | The `#[actor(ts)]` macro codegen uses uniform serde paths (no type-matching) |
| T7 | Prerequisite: graceful serde migration (PEER.try_get() dispatch, not Encode/Decode) |
| T8 | `TsActorRef` gains `inner_ref()`, `JsCast`, `Into<JsValue>` bounds |
| T9 | React hooks rewritten to use generated XRef types directly |
| T10 | Dead generic TS-side code (ActorRef, CachedStream, ActorDescriptor) removed |
