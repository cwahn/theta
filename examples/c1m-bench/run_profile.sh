#!/usr/bin/env bash
set -euo pipefail

# C1M profiling benchmark runner
# Usage: ./examples/c1m-bench/run_profile.sh [N1 N2 ...]
# Default: 100000 200000 500000 1000000

COUNTS="${@:-100000 200000 500000 1000000}"
BIN_DIR="./target/release"
SERVER="$BIN_DIR/c1m_server"
PROFILER="$BIN_DIR/c1m_profile"

echo "Building release binaries..."
cargo build -p theta-c1m-bench --release 2>&1 | tail -5

for N in $COUNTS; do
    echo ""
    echo "################################################################"
    echo "  PROFILING N=$N"
    echo "################################################################"

    export MAX_STREAMS=$((N + 10))

    SAMPLE_COUNT=$((N < 2000 ? N : 2000))
    export SAMPLE_SIZE=$SAMPLE_COUNT

    # Start server
    RUST_LOG=warn,theta=warn $SERVER "$N" > /tmp/c1m_server_output.txt 2>&1 &
    SERVER_PID=$!

    # Wait for public key
    PK=""
    for i in $(seq 1 60); do
        if [ -f /tmp/c1m_server_output.txt ]; then
            PK=$(grep -o 'PUBLIC_KEY:.*' /tmp/c1m_server_output.txt 2>/dev/null | head -1 | cut -d: -f2 || true)
            if [ -n "$PK" ]; then
                break
            fi
        fi
        sleep 0.5
    done

    if [ -z "$PK" ]; then
        echo "ERROR: Server did not produce a public key within 30s"
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        cat /tmp/c1m_server_output.txt 2>/dev/null || true
        continue
    fi

    echo "Server PID=$SERVER_PID, N=$N, MAX_STREAMS=$MAX_STREAMS"

    SPAWN_WAIT=$((N / 100000 + 2))
    echo "Waiting ${SPAWN_WAIT}s for server to be ready..."
    sleep "$SPAWN_WAIT"

    # Run profiler
    RUST_LOG=warn,theta::remote::peer=error $PROFILER "$PK" 2>&1 || true

    # Collect server memory
    SERVER_MEM=$(ps -o rss= -p "$SERVER_PID" 2>/dev/null || echo "0")
    SERVER_MEM_MB=$(echo "scale=1; $SERVER_MEM / 1024" | bc)
    echo ""
    echo "[Server Memory] PID=$SERVER_PID RSS=${SERVER_MEM_MB} MB"

    # Cleanup
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    rm -f /tmp/c1m_server_output.txt

    sleep 2
done

echo ""
echo "All profiling complete."
