# Import ACK Handshake — Fixing iroh Relay→Direct Path Migration Data Loss

> Temporary working document. Delete after implementation is complete.

## Problem

iroh 0.97.0 loses data during relay→direct path migration. When a QUIC connection migrates from relay to direct UDP (~100ms after connection), data sent on bi-streams during the migration window is permanently lost. The server's `accept_bi()` returns (stream metadata arrives) but the actual STREAM frame data never arrives. QUIC retransmission does not recover it.

**Evidence (40+ test runs at 100K scale):**

| Connection Type           | Runs | Failures | Rate   |
|---------------------------|------|----------|--------|
| Relay→Direct (~101-103ms) | 9    | 3        | **33%**|
| Already Direct (<1ms)     | 11   | 0        | **0%** |

At 1M scale, the failure rate rises to ~50% since more bi-streams are opened during the migration window.

## Root Cause

1. Client connects via **relay** (iroh's default).
2. Client opens bi-stream + sends InitFrame (22 bytes) — still on relay path.
3. ~100ms later, iroh discovers **direct** UDP path; `select_path()` switches immediately.
4. In-flight data on the relay path is lost; QUIC retransmission doesn't recover it.
5. No `flush()` method exists on SendStream — writes are fire-and-forget into quinn's buffer.

The relay path is NOT closed (`close_redundant_paths` skips relay). The loss happens at the noq/quinn multipath layer during path transition.

## Solution: Application-Level ACK Handshake

Treat iroh as **at-most-once** delivery during path migration. Add a 1-byte ACK to confirm the server received the InitFrame before the client proceeds.

### Protocol Change (bi-stream import only)

**Before:**
```
Client                          Server
  |-- open_bi() ------------------>|  accept_bi()
  |-- send_frame(InitFrame) ------>|  recv_frame(InitFrame) -> spawn_export_task
  |-- send_frame(msg1) ----------->|  recv_frame(msg1)
```

**After:**
```
Client                          Server
  |-- open_bi() ------------------>|  accept_bi()
  |-- send_frame(InitFrame) ------>|  recv_frame(InitFrame)
  |<-- ACK (0x01) -----------------| *write_all(&[0x01])*
  |   (verify ACK received)        |  spawn_export_task
  |-- send_frame(msg1) ----------->|  recv_frame(msg1)
```

### Client-Side Retry with Exponential Backoff

If ACK is not received within the timeout, the client drops the bi-stream and retries. Timeouts escalate:

```
100ms → 250ms → 500ms → 1s → 2.5s → 5s → fail
```

6 attempts total. Each attempt opens a fresh bi-stream.

## Files Changed

| File | Change |
|------|--------|
| `theta/src/remote/peer.rs` (accept loop) | After `recv_frame(InitFrame)` + actor lookup, send `0x01` ACK byte before spawning export task |
| `theta/src/remote/peer.rs` (import task) | Wrap `open_bi → send_frame → wait_ack` in retry loop with exponential backoff |

No changes to: `network.rs`, `actor_ref.rs`, `message.rs`, frame types.

## Edge Cases

- **Duplicate export tasks on retry**: Old bi-stream dropped → server export task gets recv error → terminates. Brief overlap harmless (separate stream pairs).
- **Monitor path**: Uses uni-streams (no ACK possible). Not affected — out of scope.
- **Relay-only connections**: ACK works identically, just higher RTT. 100ms first timeout may be tight but subsequent retries cover it.
- **Server recv timeout**: Keep existing 5s `recv_frame_into` timeout as safety net for orphaned streams.
