# Plan: Theta Web/WASM Support + Browser Chat Example

## TL;DR
Add WASM/browser support to theta by creating a `compat` module that abstracts platform-divergent APIs (`tokio::spawn`, `tokio::select!`, `tokio::time`, `DashMap`, `task_local!`), then build a browser chat example using wasm-pack + plain HTML/wasm-bindgen that demonstrates theta actors working in a browser via iroh relay.

**Branch**: `feature/web-support`

---

## Phase 1: Build Infrastructure

### Step 1.1 — Add `build.rs` to theta crate
- Create `theta/build.rs` using `cfg_aliases` crate (same pattern as iroh)
- Define `wasm_browser: { all(target_family = "wasm", target_os = "unknown") }`
- Add `cfg-aliases` to `[build-dependencies]` in `theta/Cargo.toml`

### Step 1.2 — Update `theta/Cargo.toml` dependencies
- Split `tokio` into target-conditional:
  - Native: `tokio = { features = ["sync", "rt", "time", "macros", "rt-multi-thread", "signal"] }`
  - WASM: `tokio = { features = ["sync"] }` (just channels/mutexes)
- Add WASM-specific deps:
  - `wasm-bindgen-futures = "0.4"` (for spawn_local)
  - `wasm-bindgen = "0.2"` (for JS interop)
  - `getrandom = { version = "0.4", features = ["wasm_js"] }` (already present)
  - `gloo-timers = { version = "0.3", features = ["futures"] }` (for sleep/timeout)
- Keep existing `dashmap` conditional exclusion
- Add WASM replacement: `std::collections::HashMap` via compat module
- Add `crate-type = ["lib", "cdylib"]` for WASM output

---

## Phase 2: Compat Module (`theta/src/compat.rs`)

### Step 2.1 — `spawn()` wrapper
- Native: `tokio::spawn(future)` (returns JoinHandle, but callers ignore it)
- WASM: `wasm_bindgen_futures::spawn_local(future)` 
- All 14 call sites use fire-and-forget pattern, so return type can diverge via cfg

### Step 2.2 — `select!` alternative
- Both usages in theta are 2-branch biased selects (actor_instance.rs, peer.rs)
- Replace both with `futures::future::select(pin!(a), pin!(b))` + `Either` match

### Step 2.3 — Time utilities
- `compat::sleep(duration)` → `tokio::time::sleep` / `gloo_timers::future::sleep`
- `compat::timeout(duration, future)` → `tokio::time::timeout` / manual impl with `futures::select`
- Affects: peer.rs (2 sites), actor_ref.rs (1 site), base.rs (Elapsed type)

### Step 2.4 — `ConcurrentMap<K, V>` type alias
- Native: `DashMap<K, V>` (concurrent hashmap with sharding)
- WASM: `std::sync::Mutex<HashMap<K, V>>` (single-threaded, Mutex is cheap)
- Wrap with consistent API: `get()`, `insert()`, `remove()`, `entry()`
- Affects: ~20 usages in context.rs (BINDINGS), peer.rs (peers, imports, pending_*)

### Step 2.5 — `task_local!` replacement
- Native: keep `tokio::task_local!` as-is
- WASM: use `thread_local!` + `RefCell` with scope function (single-threaded safety)
- Only 1 definition (PEER in peer.rs) + 5 scope usages

---

## Phase 3: Core WASM Adaptation

### Step 3.1 — Replace `tokio::spawn` calls (14 sites)
- `theta/src/context.rs` (2 sites)
- `theta/src/actor_ref.rs` (4 sites)
- `theta/src/remote/peer.rs` (6 sites)
- `theta/src/remote/serde.rs` (2 sites)

### Step 3.2 — Replace `tokio::select!` calls (2 sites)
- `theta/src/actor_instance.rs` — signal vs message priority
- `theta/src/remote/peer.rs` — incoming message vs disconnect

### Step 3.3 — Replace `tokio::time` calls (4 sites)
- `theta/src/remote/peer.rs` (2 sites) — connection timeouts
- `theta/src/actor_ref.rs` (1 site) — request timeout
- `theta/src/remote/base.rs` — Elapsed error type import

### Step 3.4 — Replace DashMap usage (~20 refs)
- `theta/src/context.rs` — BINDINGS global registry
- `theta/src/remote/peer.rs` — peers, imports, pending_recv_replies, pending_lookups, pending_monitors

### Step 3.5 — Replace `task_local!` (1 def + 5 uses)
- `theta/src/remote/peer.rs` — PEER definition + 1 scope usage
- `theta/src/actor_ref.rs` — 3 scope usages
- `theta/src/remote/serde.rs` — 1 scope usage

### Step 3.6 — Handle `tokio::io` traits
- `theta/src/remote/network.rs` — `AsyncReadExt`, `AsyncWriteExt`

---

## Phase 4: Browser Chat Example

### Project structure
```
examples/browser-chat/
  Cargo.toml          # workspace with 3 members
  shared/
    Cargo.toml        # lib crate, depends on theta
    src/lib.rs        # ChatRoom actor, SendMessage, GetHistory messages
  server/
    Cargo.toml        # bin crate, depends on shared + tokio + iroh
    src/main.rs       # Host ChatRoom, print public key, listen
  browser/
    Cargo.toml        # cdylib crate, depends on shared + wasm-bindgen
    src/lib.rs        # WASM exports: connect, send_message, get_state
    index.html        # Chat UI (vanilla HTML/CSS/JS)
    build.sh          # wasm-pack build --target web
```

### Chat actor
- `ChatRoom` actor with `type View = Vec<ChatMessage>`
- Messages: `SendMessage { author, text }` and `GetHistory`
- `ChatMessage { author, text, timestamp }` (serde)
- Browser monitors ChatRoom state for real-time updates

---

## Phase 5: Verification

1. `cargo build --target wasm32-unknown-unknown -p theta --no-default-features --features "macros,remote,monitor"` — must compile
2. `wasm-tools print --skeleton ... | grep 'import "env"'` — must find nothing
3. `cargo test -p theta` — existing native tests still pass
4. End-to-end: native server + browser client chatting via iroh relay

---

## Key Decisions
- **tokio::select! → futures::future::select()** — only 2 sites, both 2-branch, avoids macro compat issues
- **DashMap → Mutex<HashMap> on WASM** — Mutex is cheap single-threaded; avoids RefCell borrow panics across await points
- **task_local! → thread_local! + RefCell on WASM** — correct for single-threaded where scope restores context on poll
- **Scope**: theta core + one example only. theta-macros unaffected (proc-macro). persistence/project_dir excluded from WASM (already feature-gated)
- **iroh 0.97.0**: already has full WASM support, no upgrade needed
