# Plan: Structurally Simplify Peer State Machine

## Core Insight

`pending_lookups` and `pending_monitors` exist because Lookup/Monitor use the shared control uni-stream (request on one stream, response on different stream/oneshot, need a key to correlate). If we use bi-streams for all request/response operations, **the stream itself IS the correlation**. No maps, no keys, no cleanup, no leak.

## Architecture Change

### Before (current)
- Control uni-stream: Forward + LookupReq/Resp + Monitor
- Bi-streams: Import (long-lived per-actor)
- Uni-streams (accept loop): Monitor responses
- State: pending_lookups, pending_monitors, next_key, move_map
- PeerInner: 5+ fields (public_key, conn, next_key, pending_lookups, pending_monitors, favored_peer)

### After (proposed)
- Control uni-stream: Forward only (fire-and-forget)
- Bi-streams: Import (long-lived) + Lookup (short-lived) + Monitor (long-lived)
- Uni-streams: Not accepted in loop (control_rx accepted once at setup)
- State: NONE (correlation state eliminated)
- PeerInner: 3 fields (public_key, conn, favored_peer)

## What Gets Deleted (~200 lines)
- `pending_lookups: ConcurrentMap` + type alias
- `pending_monitors: ConcurrentMap` + type alias
- `next_key: AtomicU32`
- `move_map()` method + calls in peer replacement
- `ControlFrame::LookupReq`, `ControlFrame::LookupResp`, `ControlFrame::Monitor`
- `process_lookup_req()`, `process_lookup_resp()`, `process_monitor()` methods
- `open_uni_with()` method
- `spawn_recv_frame()` method (folded into main loop)
- `next_key()` method
- `accept_uni` in main loop (only Monitor used it)

## Phases

### Phase 1: Structural Simplification
Move Lookup and Monitor to bi-streams. Fold control_rx into main select loop.

**InitFrame grows two variants:**
```
InitFrame::Lookup { actor_ty_id, ident }
InitFrame::Monitor { actor_ty_id, ident }  // cfg(feature = "monitor")
```

**Lookup (new flow):**
- Client: open_bi → send InitFrame::Lookup → timeout(5s, recv_frame) → deserialize → close
- Server: accept_bi → spawn handler → recv InitFrame → lookup actor → send response → close
- No pending map. No key. Natural timeout. Auto-cleanup on stream close.

**Monitor (new flow, cfg(feature = "monitor")):**
- Client: open_bi → send InitFrame::Monitor → use recv_half for updates (long-lived)
- Server: accept_bi → spawn handler → recv InitFrame → lookup → send error or push updates on send_half
- No pending map. No oneshot. No key. Error sent on same stream.

**Main loop simplification (2-way select instead of 3-way):**
```rust
loop {
    select(control_rx.recv_frame(), self.conn.accept_bi()) {
        Left(ctrl) => handle_forward(ctrl),        // only Forward now
        Right(bi) => spawn_bi_handler(bi),          // Import/Lookup/Monitor
    }
}
```
- When control_rx dies → loop exits → remove_peer (death detection for free)
- No asymmetric half-dead state

**Peer replacement simplification:**
- Delete move_map calls and next_key transfer entirely
- In-flight lookups on old connection: they have their own bi-stream, either complete or timeout naturally

### Phase 2: Import ACK + Graduated Retry
- Server: after recv InitFrame::Import + deserialize, write raw `0x01` byte on send_half before spawn_export_task
- Client: graduated timeout ACK retry [100, 250, 500, 1000, 2500, 5000] ms
  - Each retry: close old streams → open_bi → send InitFrame → timeout(T, read 1 byte)
  - On final failure: remove_actor and return
- **Double-import idempotency**: Server spawns export handler for each accepted bi-stream. When client retries, it drops the old stream → QUIC RESET_STREAM → old export handler exits. New handler takes over. Natural cleanup.

### Phase 3: Export Reply Resilience
- In spawn_export_task, ContinuationDto::Reply arm: if reply_stream.send_frame fails, warn + continue instead of break
- Ask caller times out. Tells continue to be served.

## Files Modified
- `theta/src/remote/peer.rs` — Phase 1 (major), Phase 2
- `theta/src/actor_ref.rs` — Phase 3

## Verification
1. `cargo build --release`
2. `cargo build --release --target wasm32-unknown-unknown -p theta` (WASM)
3. `cargo test`
4. `stress_hang.sh 20 1000000 120`

## Decisions
- QUIC guarantees ordered delivery per stream (RFC 9000 §2.2) → Reply FIFO safe as-is
- Lookup: single 5s timeout (caller retries externally)
- Import: graduated retry (internal critical operation)
- Export: skip failed replies, continue tells
- ControlFrame reduced to Forward-only (or eliminated if Forward mechanism changes)
