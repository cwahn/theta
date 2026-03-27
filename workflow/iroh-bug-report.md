# iroh Bug Report: Data Loss During Relay→Direct Path Migration

## Summary

Bi-directional stream data is permanently lost during iroh's relay-to-direct path migration. The server's `accept_bi()` succeeds (stream metadata arrives) but STREAM frame data sent immediately after `open_bi()` never reaches the receiver. QUIC retransmission does not recover the data.

## Environment

- **iroh**: 0.97.0
- **noq**: 0.17.0
- **OS**: macOS (Apple Silicon)
- **Network**: Both peers on same LAN (relay→direct migration takes ~100ms)

## Reproduction

### Setup
Two peers on the same network. Peer A (server) spawns N actors and accepts bi-streams. Peer B (client) connects to Peer A and opens N bi-streams, each carrying a 22-byte InitFrame.

### Steps
1. Server binds endpoint, spawns 100,000 actors
2. Client connects to server (initially via relay)
3. Client opens bi-streams and sends InitFrame (22 bytes each) via `write_all_chunks(&[len_bytes, data_bytes])`
4. Server calls `accept_bi()`, then `read_exact()` for 4-byte length prefix + payload
5. ~100ms after connection, iroh discovers direct UDP path and migrates

### Expected
All bi-streams successfully deliver their InitFrame data.

### Actual
~33% of runs that experience relay→direct migration lose data on one or more streams:
- Server's `accept_bi()` returns successfully (stream metadata delivered)
- Server's `read_exact()` for the 4-byte length prefix blocks indefinitely
- Adding a 5-second timeout confirms the data never arrives
- Client's `write_all_chunks()` returns `Ok(())` — the write appeared successful

### Evidence (40+ automated test runs at 100K scale)

| Direct Connection Timing | Total Runs | Failures | Rate |
|--------------------------|-----------|----------|------|
| ~101-103ms (relay→direct migration) | 9 | 3 | **33%** |
| <1ms (already direct, no migration) | 11 | 0 | **0%** |

Key observations:
- **100% correlation** between failures and relay→direct migration timing
- **0 failures** when connection starts direct (no migration occurs)
- Data loss is permanent — not recovered by QUIC retransmission
- `SendStream::stopped()` does NOT fire on the client — stream appears healthy
- Adding artificial delay before data flow (waiting for direct path) eliminates failures

### At 1M scale
Failure rate rises to ~50%, presumably because more bi-streams are opened during the migration window.

## Analysis

Looking at iroh source:
- `select_path()` switches the selected path immediately with no grace period
- `close_redundant_paths()` does NOT close the relay path (filters `!addr.is_relay()`)
- No data flushing occurs before the path switch
- noq is single-path aware; iroh intercepts at the socket layer for multipath routing
- The data appears to be "orphaned" — written to the old relay path's quinn buffer but routed/retransmitted on the new direct path where the QUIC connection ID may not correlate

## Related Issues

- #3092: Relay data loss (fixed in v0.31.0 by PR#3062/#3099) — different mechanism (relay-only mode, attributed to effective deadlocks in socket actors), but same symptom of permanent data loss in iroh transport.

## Workaround

Application-level ACK handshake: server sends 1-byte ACK after receiving InitFrame, client retries on timeout with exponential backoff (100ms→250ms→500ms→1s→2.5s→5s). This treats iroh as at-most-once delivery during path migration and recovers automatically.
