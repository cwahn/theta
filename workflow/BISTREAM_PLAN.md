# Per-Actor Bidirectional QUIC Streams — Implementation Plan

> Temporary working document. Delete after implementation is complete.

## Problem

`control_tx: Arc<Mutex<TxStream>>` — single shared stream for all reply traffic per peer connection.
Profiling confirmed 99.9–100% of `send_control_frame` time is mutex wait (46ms avg at N=10K, 514ms at N=100K).

## Solution

Each import/export actor pair gets a QUIC bi-stream instead of a uni-stream.
- SendHalf: messages (same as current uni-stream)
- RecvHalf: replies (new)

Replies correlated by FIFO order — no DashMap, no Key.
Server export task is synchronous: read msg → send to actor → await reply → write reply to stream.
Control stream (mutex) kept for Forward/Lookup/Monitor — now uncontended.

## Decisions

| Decision | Choice |
|---|---|
| Reply channel | oneshot per reply |
| Reply queue (client) | flume FIFO of pending oneshot senders |
| Key correlation for replies | Removed (FIFO order) |
| Forward/Lookup/Monitor | Stay on control stream |
| Buffer strategy | Fresh allocation each recv |
| Network abstraction | Removed — use iroh SendStream/RecvStream directly |
| Export task model | Synchronous: send → await → write |

## Phase 1: Flatten Network Abstraction

### network.rs

- Remove `TxStream`, `RxStream`, `Transport` wrapper structs
- Keep `NetworkError` (Clone-able iroh error wrappers)
- Keep `Network` (Endpoint, connect/accept)
- Add extension traits on iroh `SendStream`/`RecvStream`:
  - `SendFrameExt::send_frame(&mut self, data: &[u8]) -> Result<(), NetworkError>`
  - `RecvFrameExt::recv_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<(), NetworkError>`
- `PreparedConnInner`: `conn: Connection`, `control_tx: Arc<Mutex<SendStream>>`
- Add `open_bi()`, `accept_bi()` on PreparedConn
- Keep `open_uni()`, `accept_uni()` (Monitor still needs them)

### Callers

- `AnyActorRef::spawn_export_task`: `(Peer, RecvStream, SendStream)` instead of `(Peer, RxStream)`
- `AnyActorRef::monitor_as_bytes`: `SendStream` instead of `TxStream`
- All `TxStream`/`RxStream` in peer.rs → `SendStream`/`RecvStream`

## Phase 2: Bi-Stream Import/Export

### peer.rs import() — Client side

- `open_bi()` → `(send_half, recv_half)`
- Send `InitFrame` on `send_half`
- Reply queue: `flume::unbounded::<oneshot::Sender<(Peer, Vec<u8>)>>()`
- Spawn reply reader on `recv_half`:
  ```
  loop { recv_frame → pop FIFO → oneshot.send((peer, bytes)) }
  ```
- Handle `Continuation::Reply` before `into_dto()`:
  - Create double-oneshot locally, push `bin_reply_tx` to FIFO
  - Return `ContinuationDto::Reply` (marker, no Key)

### peer.rs run() — Server accept loop

- `select(accept_bi, accept_uni)`:
  - bi → Import: `spawn_export_task(peer, recv, send)`
  - uni → Monitor: existing handling

### actor_ref.rs spawn_export_task — Server synchronous loop

```
loop {
  recv_frame → deserialize (msg, k_dto)
  match k_dto {
    Reply → oneshot → BinReply { peer, reply_tx } → send to actor → await reply → send_frame
    Nil → send with Nil, continue
    Forward → existing From conversion, send, continue
  }
}
```

## Phase 3: Wire Reply Path

### message.rs

- `BinReply { peer: Peer, key: Key }` → `BinReply { peer: Peer, reply_tx: oneshot::Sender<Vec<u8>> }`

### serde.rs

- `ContinuationDto::Reply(Key)` → `ContinuationDto::Reply` (no payload)
- `into_dto()`: panic on Reply ("handled at import site")
- `From<ContinuationDto>`: panic on Reply ("handled in export task")

### peer.rs — Remove reply infrastructure

- Remove from `PeerInner`: `pending_recv_replies` DashMap
- Remove: `arrange_recv_reply()`, `send_reply()`, `process_reply()`
- Remove: `ControlFrame::Reply` variant + match arm

### theta-macros/actor.rs

- `BinReply { peer, reply_tx }` → `let _ = reply_tx.send(bytes);` (sync, no I/O)

## Phase 4: Cleanup & Verify

- Error propagation is natural: actor panic → reply_tx dropped → export breaks → bi-stream closes → reply reader breaks → caller sees RequestError
- Update perf-instrument counters
- `cargo test --all-features`
- Profile at N=10K, N=100K
- Target N=1M: 0% failures, >30K rt/s

## FIFO Correctness Invariant

1. Actor processes messages sequentially (actor_instance.rs process_msg)
2. QUIC bi-stream preserves order (protocol guarantee)

Therefore: replies arrive in exact order of asks. No key correlation needed.

## Wire Format Break

`ContinuationDto::Reply` loses its `Key` field. Incompatible with older peers. Acceptable: pre-1.0 alpha.
