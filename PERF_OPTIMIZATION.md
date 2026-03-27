# Theta Remote — Performance Optimization & Stabilization

> **Temporary working document** for the `c1m-optimization` branch.

---

## 1. Benchmark Results (profiling binary, local relay)

All tests: two local processes (host + client), iroh QUIC via relay, `MAX_STREAMS=1100000`.

| N | Seq rt/s | Wave-N rt/s | Wave-N CV% | Wave-N failures | Tell msgs/s | Client MB | Server MB |
|---|---------|------------|-----------|----------------|------------|----------|----------|
| 100K | 7,805 | 29,141 | 73.9% | 0 | 2,170,000 | 243 | 399 |
| 200K | 9,001 | 31,427 | 58.4% | 0 | 827,000 | 444 | 522 |
| 500K | 6,574 | 16,848 | 61.3% | 0 | 626,000 | 1,105 | 574 |
| 1M | 4,211 | 9,978 | 62.6% | 350,434 (35%) | 229,000 | 1,451 | 77* |

*\*Server 77 MB at 1M likely killed before measurement.*

### Key latency observations

- **Sequential between-batch CV is low (5–18%)** — steady-state per-ask latency is consistent.
- **Sequential batch p99 CV jumps** from 10% (100K) to 59% (500K–1M) — intermittent spikes.
- **Concurrent wave p99 explodes**: ~3 s at N=100K → 29 s at N=500K → 58 s at N=1M.
- **1M concurrent: 35% failures** — likely 60 s timeout exceeded.
- **Tell throughput degrades 10×**: 2.17M → 229K msgs/s from 100K to 1M.

---

## 2. Code-Review: Full Ask Round-Trip Path

### 2.1 Client → server (send path)

```
actor_ref.ask(Ping)
  └─ oneshot(reply_tx, reply_rx) created
  └─ flume::send((Ping, Continuation::Reply(reply_tx)))           ← per-actor unbounded flume
       └─ import task loop (one per actor)                        ← dedicated tokio task
            ├─ k.into_dto()                                       ← creates ANOTHER oneshot pair
            │    ├─ peer.arrange_recv_reply(bin_reply_tx)          ← DashMap::insert(AtomicU32 key)
            │    └─ sends oneshot::Receiver<(Peer,Vec<u8>)> back via original reply_tx
            ├─ postcard::to_stdvec(&(msg, ContinuationDto::Reply(key)))  ← allocates Vec
            └─ out_stream.send_frame(&bytes)                      ← per-actor TxStream, NO contention
                 └─ write_all(4-byte len) + write_all(data)
```

**Observations — send path:**
- Each import task has its own dedicated QUIC unidirectional stream → **no send-side contention**.
- Per-ask allocation: 2× oneshot channels, Box::pin, postcard Vec, Box<dyn Any>.
- `try_recv()` before `futures::select(recv, stopped)` is micro-optimization note ("~1-3 ns").

### 2.2 Server — message processing (middle)

```
peer.run()
  └─ accept_uni() loop
       └─ recv InitFrame::Import { actor_id }
            └─ actor.spawn_export_task(peer, in_stream)
                 └─ loop: recv_frame_into(&mut buf) on per-actor RxStream
                      ├─ postcard::from_bytes::<MsgPackDto<A>>(&buf)
                      ├─ k_dto.into() → Continuation
                      └─ this.send(msg, k)                        ← into actor's flume channel
```

**Observations — server processing:**
- Each imported actor gets its own export task with a dedicated `RxStream` → **no recv-side contention either**.
- `buf` is reused across loop iterations → minimal alloc on recv path.
- `this.send(msg, k)` goes into the actor's local flume and is processed by the actor's own task.

### 2.3 Server → client (reply path) 🔴 **CRITICAL BOTTLENECK**

```
actor processes message → reply bytes
  └─ peer.send_reply(key, reply_bytes)
       └─ send_control_frame(ControlFrame::Reply { key, reply_bytes })
            ├─ postcard::to_stdvec(&frame)                        ← allocates Vec per reply
            └─ self.0.conn.send_frame(&bytes)                     ← PreparedConn::send_frame
                 └─ inner.control_tx.lock().await                ← 🔴 Arc<futures::lock::Mutex<TxStream>>
                      └─ tx_stream.send_frame(&bytes)
                           ├─ write_all(4-byte len)
                           └─ write_all(data)
```

**This is the primary bottleneck:**

There is a **single `Arc<Mutex<TxStream>>`** (`control_tx`) shared across the entire peer connection.
ALL reply frames, lookup requests/responses, forward messages, and monitor frames are serialized
through this one mutex. At 1M concurrent asks, 1M reply frames contend for this single lock.

**The asymmetry explains the scaling wall:**
- **Sends** scale well: each actor has a dedicated unidirectional stream (no contention).
- **Replies** don't scale: all replies funnel through one shared control stream under one mutex.

Additionally, `TxStream::send_frame()` performs **two separate `write_all` calls** per frame 
(length prefix + data), doubling the lock-hold time and syscall overhead.

### 2.4 Client — reply receipt

```
spawn_recv_frame(control_rx)
  └─ loop: control_rx.recv_frame_into(&mut buf)                  ← single reader, no contention
       └─ postcard::from_bytes::<ControlFrame>(&buf)
            └─ match ControlFrame::Reply { key, reply_bytes }
                 └─ peer.process_reply(key, reply_bytes)
                      ├─ self.0.pending_recv_replies.remove(&key) ← DashMap::remove
                      └─ reply_bytes_tx.send((peer, reply_bytes)) ← oneshot fires
                           └─ back in ask() future:
                                └─ postcard::from_bytes::<Resp>(&bytes) ← final deser
```

**Observations — reply receipt:**
- Single control_rx reader task → no contention on recv.
- DashMap remove is O(1) amortized but contended at high concurrency.
- Each reply crosses 2 oneshot boundaries (bin_reply → user reply).

---

## 3. Identified Bottlenecks (Priority Order)

### P0 — Control stream mutex (reply serialization)

**File:** `theta/src/remote/network.rs:253-256`
```rust
inner.control_tx.lock().await.send_frame(data).await
```

All outbound control frames share a single `Arc<Mutex<TxStream>>` per peer connection.
At scale, 1M+ reply frames contend for this lock. The lock is held across two `write_all` syscalls
(length prefix + payload), which includes I/O wait time — potentially hundreds of microseconds per frame.

**Impact:** This is the #1 bottleneck. It explains why concurrent throughput degrades from ~29K rt/s
at N=100K to ~10K rt/s at N=1M, and why 35% of 1M asks timeout.

**Potential fixes:**
1. **Per-actor reply streams:** Open a dedicated unidirectional stream per actor for replies 
   (mirrors the import stream design). Eliminates contention entirely.
2. **Reply batching/coalescing:** Buffer replies and flush periodically, reducing lock acquisitions.
3. **Sharded control streams:** Open N control streams and shard by key hash.
4. **Lock-free MPSC → single writer:** Have reply tasks send frames into a channel; a single writer
   task drains and writes without mutex contention on the QUIC stream.

### P1 — Two write_all calls per frame

**File:** `theta/src/remote/network.rs:206-214`
```rust
self.0.write_all(&(data.len() as u32).to_be_bytes()).await?;
self.0.write_all(data).await?;
```

Each frame requires two separate async write calls. Under the mutex, this doubles the time the lock
is held and doubles syscall overhead.

**Fix:** Prepend the 4-byte length to the data buffer before writing, or use `write_all_vectored` / 
`write_vectored` (iovec-style) to send both in one syscall.

### P2 — Per-ask allocation overhead

Each ask allocates:
- `Box::pin(async { ... })` for the future
- 2× `oneshot::channel()` (one for user reply, one for binary bytes)
- `postcard::to_stdvec()` → `Vec<u8>` for serialization
- `Box<dyn Any>` for type erasure

At 1M scale, that's millions of small allocations. Not the primary bottleneck but contributes
to GC pressure and memory fragmentation.

**Fix:** Object pooling for serialization buffers; consider reducing to 1 oneshot channel.

### P3 — PreparedConn::get() clones a Shared<BoxFuture>

**File:** `theta/src/remote/network.rs:281-283`
```rust
async fn get(&self) -> Result<PreparedConnInner, NetworkError> {
    self.inner.clone().await
}
```

Every `send_frame`, `open_uni`, `accept_uni` call clones and polls a `Shared<BoxFuture>`.
After the first resolution, `Shared` returns the cached result, but the clone + poll overhead
is non-zero per call.

**Fix:** After first resolution, store the `PreparedConnInner` directly (e.g., `OnceLock`).

### P4 — DashMap contention on pending_recv_replies

At 1M scale, the `pending_recv_replies: DashMap<Key, oneshot::Sender>` sees 1M inserts
(on arrange_recv_reply) and 1M removes (on process_reply). DashMap uses 16 shards by default.
With AtomicU32 keys, distribution should be good, but 1M/16 = 62.5K ops per shard is non-trivial.

**Observation:** This is likely a minor contributor compared to P0 but may show up in profiling.

### P5 — `spawn_recv_frame` allocates a new Vec per control frame

**File:** `theta/src/remote/peer.rs:617`
```rust
let mut buf = Vec::new(); // ephemeral buffer
```

Unlike the export task (which reuses `buf`), the control frame receiver allocates a fresh `Vec`
for every frame. At 1M replies, that's 1M allocations.

**Fix:** Move `buf` outside the loop and `buf.clear()` after each iteration.

### P6 — postcard::to_stdvec allocates per frame

Each `send_control_frame` call does `postcard::to_stdvec(&frame)` which allocates a new `Vec`.

**Fix:** Use `postcard::to_slice()` with a reusable buffer, or `postcard::to_extend()` 
to serialize into a pre-allocated buffer.

---

## 4. Memory Scaling

| N | Client MB | Server MB | Per-actor (client) |
|---|----------|----------|-------------------|
| 100K | 243 | 399 | ~2.4 KB |
| 200K | 444 | 522 | ~2.2 KB |
| 500K | 1,105 | 574 | ~2.2 KB |
| 1M | 1,451 | — | ~1.5 KB |

Per-actor overhead is ~2 KB on the client side (ActorRef + flume + import task + oneshot state).
At 1M actors, client consumes ~1.5 GB — this is expected and not an optimization priority.

---

## 5. Profiling Results — Targeted Timing Instrumentation

### Technique

Added `perf-instrument` feature flag to theta. Atomic counters in:
- `send_control_frame`: serialize time, mutex wait time, total time
- `PreparedConn::send_frame`: mutex acquire time, write time
- `TxStream::send_frame`: `write_all` for length prefix vs data
- `arrange_recv_reply` / `process_reply`: DashMap insert/remove time

### Server-side results (where replies are sent)

| Scale | Calls | Avg mutex wait | Avg serialize | Avg write | Mutex % of total |
|-------|-------|---------------|--------------|-----------|-----------------|
| 10K | 27,112 | **46.3 ms** | 0.1 µs | 9.2 µs | **99.9%** |
| 100K | 267,112 | **514.4 ms** | 0.1 µs | 7.3 µs | **100.0%** |

- Mutex wait scales ~11× for 10× more workers — roughly **O(N)**
- At 100K: total cumulative mutex wait = 137,400 seconds (38 hours across all tasks)
- Average reply spends **514 ms** just waiting for the lock, while actual serialization + I/O takes < 10 µs

### Client-side results (where messages are sent)

| Scale | TxStream calls | Avg len_prefix write | Avg data write | DashMap insert avg | DashMap remove avg |
|-------|---------------|---------------------|---------------|-------------------|-------------------|
| 10K | 27,360 | 10.9 µs | 7.4 µs | 0.3 µs | 0.2 µs |
| 100K | 298,406 | 10.4 µs | 6.4 µs | 0.4 µs | 0.3 µs |

- Client-side `send_control_frame` calls: 0 in concurrent phase (client doesn't send replies)
- Per-actor import stream writes: stable 7–11 µs per `write_all` — **no contention** (confirms design)
- DashMap operations: sub-microsecond — **not a bottleneck** (P4 is deprioritized)

### Conclusion

The **single `control_tx` mutex is THE bottleneck**, consuming 99.9–100% of reply send time.
All other suspected bottlenecks (DashMap, serialization, write syscalls) are negligible in comparison.

---

## 6. Optimization Plan

### Phase 1: Fix P0 — Eliminate control stream mutex contention

**Goal:** Replace single `Arc<Mutex<TxStream>>` with contention-free reply path.

**Recommended approach: MPSC channel → single writer task**

```
Actor reply → peer.send_reply(key, bytes)
  └─ mpsc::send((key, bytes))       ← non-blocking channel send
       └─ writer task (single):
            ├─ recv_many → drain channel
            ├─ for each: serialize + write_all to control stream
            └─ no mutex needed (single owner of TxStream)
```

Why this over per-actor reply streams:
- Per-actor reply streams would require 2× the QUIC streams (one import + one reply per actor)
- At 1M actors: 2M QUIC streams. QUIC connection overhead grows.
- MPSC channel + single writer is simpler, uses 1 stream, and eliminates contention.

### Phase 2: Fix P1 — Combine length + data writes

Batch the 4-byte length prefix with the data into a single `write_all`:
```rust
let mut frame = Vec::with_capacity(4 + data.len());
frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
frame.extend_from_slice(data);
self.0.write_all(&frame).await?;
```

Or better, in the writer task, batch multiple frames into one large write.

### Phase 3: Evaluate remaining optimizations

After Phase 1+2, re-benchmark at N=100K and N=1M. If throughput at 1M > 50K rt/s with
< 5% failures, we're done. If not, investigate:
- P2: Reduce per-ask allocations (object pooling, single oneshot)
- P3: Replace `Shared<BoxFuture>` with `OnceLock` for PreparedConn
- P5: Reuse buf in `spawn_recv_frame` loop
- P6: Use `postcard::to_extend()` for serialization

### Success criteria

- Wave-100K: > 50K rt/s, 0 failures, p99 < 1s
- Wave-1M: > 30K rt/s, < 1% failures, p99 < 10s
- Tell-1M: > 1M msgs/s
