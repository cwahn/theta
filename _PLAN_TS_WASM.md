# Plan: TS/WASM Integration for Theta

> Temporary documentation. Delete after implementation is complete.

## Architecture

```
React Layer (theta-ts/react)           — useActorRef<A>, useActorState<A>
JS/TS Runtime (theta-ts/core)          — ActorRef<A>, CachedStream<T>
WASM Boundary (proc macro generated)   — {Actor}Handle, spawn/lookup/bind
TS Types (typescript_custom_section)   — Msg, Returns, View, Descriptor
Theta Runtime + theta-wasm crate       — init, root context, prelude
```

## Phases

### Phase 1: theta-wasm Rust crate
- `theta-wasm/Cargo.toml` — deps: theta, theta-flume, wasm-bindgen, serde-wasm-bindgen, js-sys
- `theta-wasm/src/lib.rs` — OnceLock<RootContext>, initTheta(), initThetaLocal(), prelude re-exports
- Add to workspace members

### Phase 2: Proc macro extension
- Add `ts: bool` to ActorArgs in theta-macros/src/actor.rs
- When ts=true + cfg(wasm32), generate:
  - typescript_custom_section: {Actor}Msg, {Actor}Returns, {Actor}View, {Actor} descriptor
  - {Actor}__TsMsg enum (externally tagged) + From conversion
  - {Actor}Handle wasm-bindgen class: tell, ask, prep, initStream
  - Free functions: spawn{Actor}, lookup{Actor}, lookup{Actor}Local, bind{Actor}
- New `#[message(ts)]` attribute macro (theta-macros/src/message_ts.rs):
  - Named structs → TS interface
  - Unit/empty structs → Record<string, never>
  - Type mapping: String→string, bool→boolean, i8-i32/u8-u32/f32/f64→number, i64/u64→bigint, Vec<T>→T[], Option<T>→T|null, HashMap→Record

### Phase 3: theta-ts core (TypeScript)
- CachedStream<T> — value, subscribe(listener)→unsub, _push(value)
- ActorRef<A extends ActorDescriptor> — from(), tell(), ask(), subscribe()
  - Static Map<string, Promise<CachedStream>> for global caching
  - WasmActorHandle interface (module-private)

### Phase 4: theta-ts React hooks
- useActorRef<A>(lookupFn, deps) → ActorRef<A> | null
- useActorState<A>(ref) → A["View"] | null (via useSyncExternalStore)

### Phase 5: Browser-chat migration (future, not in this commit)

## Key Design Decisions

- Single descriptor type param: `ActorRef<ChatRoom>` not `ActorRef<Msg, View, Returns>`
- Stream cache stores Promises (not resolved values) to prevent race
- ask() uses full enum deserialization, not Reflect key extraction
- Externally tagged serde for message enum
- Lookup ≠ Subscription at React layer
