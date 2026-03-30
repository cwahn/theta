# Theta Examples

## Basics — Local actors, no networking

```sh
# Simple counter — tell() and ask() basics
cargo run -p basics --bin readme_counter

# Error handling patterns — tell vs ask error behavior
cargo run -p basics --bin tell_error
```

## Counter — Remote actors with host/client

Run in two separate terminals:

```sh
# Terminal 1: Start the host (prints public key)
cargo run -p counter --bin host

# Terminal 2: Connect as client (paste host's public key when prompted)
cargo run -p counter --bin client
```

## Monitoring — Subscribe to remote actor state

Run in two separate terminals:

```sh
# Terminal 1: Start monitored counter (auto-increments every second)
cargo run -p monitoring --bin monitored

# Terminal 2: Watch state changes (paste monitored's public key when prompted)
cargo run -p monitoring --bin monitor
```

## Ping-Pong — Peer-to-peer messaging

Each variant runs as two instances in separate terminals. Both print a public key — paste the other's key to connect.

```sh
# Interactive ping-pong (5s interval, latency printed)
cargo run -p ping-pong --bin peer

# Benchmark (100K iterations, latency percentiles)
cargo run -p ping-pong --bin bench

# Forwarding variant (messages forwarded with actor refs)
cargo run -p ping-pong --bin forward
```

## Dedup — Connection deduplication test

Self-contained — spawns a child process automatically:

```sh
cargo run -p dedup
```

## Browser Chat

See [browser-chat/](browser-chat/) for the browser-based chat example.

## C1M Bench

See [c1m-bench/](c1m-bench/) for the connection stress test.
