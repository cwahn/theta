#!/usr/bin/env bash
set -euo pipefail

# Remote C10K benchmark runner
# Usage: ./c10k/run_bench.sh [N1 N2 N3 ...]
# Default: 100 1000 5000 10000

COUNTS="${@:-100 1000 5000 10000}"
BIN_DIR="./target/release"
SERVER="$BIN_DIR/c10k_server"
CLIENT="$BIN_DIR/c10k_client"

echo "Building release binaries..."
cargo build -p theta-c10k --release 2>&1 | tail -3

for N in $COUNTS; do
    echo ""
    echo "================================================================"
    echo "  Starting remote benchmark with N=$N"
    echo "================================================================"

    # Set max_concurrent_uni_streams = N + 10 (headroom for control + manager streams)
    export MAX_STREAMS=$((N + 10))

    # Start server in background
    $SERVER "$N" > /tmp/c10k_server_output.txt 2>&1 &
    SERVER_PID=$!

    # Wait for server to print its public key
    PK=""
    for i in $(seq 1 30); do
        if [ -f /tmp/c10k_server_output.txt ]; then
            PK=$(grep -o 'PUBLIC_KEY:.*' /tmp/c10k_server_output.txt 2>/dev/null | head -1 | cut -d: -f2 || true)
            if [ -n "$PK" ]; then
                break
            fi
        fi
        sleep 0.5
    done

    if [ -z "$PK" ]; then
        echo "ERROR: Server did not produce a public key within 15s"
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        cat /tmp/c10k_server_output.txt
        continue
    fi

    echo "Server PID=$SERVER_PID, public_key=$PK"

    # Give server a moment to fully bind
    sleep 1

    # Run client
    $CLIENT "$PK" 2>&1 || true

    # Clean up server
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    rm -f /tmp/c10k_server_output.txt

    # Small pause between runs
    sleep 1
done

echo ""
echo "All benchmarks complete."
