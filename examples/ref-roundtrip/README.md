# ref-roundtrip integration tests

End-to-end tests that verify `ActorRef` serialization round-trips across the
WASM/TypeScript ↔ native Rust boundary.

## Structure

```
ref-actors/     Rust crate — test actors (compiled to WASM for TS, native for the host)
native/         Native Rust binary — boots actors and waits for connections
test-app/       Vite dev server + Playwright test suite
```

## Running locally

### 1. Build the native binary

```bash
cargo build -p ref-roundtrip-native
```

### 2. Install Node dependencies + Playwright browser

```bash
cd examples/ref-roundtrip/test-app
npm install
npx playwright install chromium
```

### 3. Run the tests

```bash
npm test
```

Playwright starts the Vite dev server (which in turn runs `wasm-pack`), launches
the native binary, opens a headless Chromium, and checks that all five round-trip
scenarios report `✓ pass`.

## Test scenarios

| ID | Description |
|----|-------------|
| T1 | `ask(SpawnItem)` returns `ActorRef<Item>` — ActorRef as return value |
| T2 | `ask(Relay { target })` — `ActorRef<Item>` as a field inside the request message |
| T3 | `ask(GetBundle)` returns `Bundle { a, b: ActorRef<Item> }` — struct with two ActorRef fields |
| T4 | `ask(GetGroup)` returns `Group { members: Vec<ActorRef<Item>> }` — Vec of ActorRefs |
| T5 | `ask(GetNested)` returns `Container { inner: Shelf { item: ActorRef<Item> } }` — nested struct |

Each scenario also validates the ref by calling `itemRef.ask({ Ping: {} })` and
asserting the expected `"pong:<label>"` response.
