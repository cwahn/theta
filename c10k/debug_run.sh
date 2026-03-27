#!/usr/bin/env bash
set -euo pipefail

# Debug run with trace logging to see what happens at N=100
N="${1:-100}"
BIN_DIR="./target/release"
SERVER="$BIN_DIR/c10k_server"
CLIENT="$BIN_DIR/c10k_client"

echo "Building release binaries..."
cargo build -p theta-c10k --release 2>&1 | tail -3

echo "Starting server with N=$N and TRACE logging..."
RUST_LOG="info,theta=trace" $SERVER "$N" > /tmp/c10k_server_trace.log 2>&1 &
SERVER_PID=$!

PK=""
for i in $(seq 1 30); do
    if [ -f /tmp/c10k_server_trace.log ]; then
        PK=$(grep -o 'PUBLIC_KEY:.*' /tmp/c10k_server_trace.log 2>/dev/null | head -1 | cut -d: -f2 || true)
        if [ -n "$PK" ]; then
            break
        fi
    fi
    sleep 0.5
done

if [ -z "$PK" ]; then
    echo "ERROR: Server did not produce a public key within 15s"
    kill "$SERVER_PID" 2>/dev/null || true
    cat /tmp/c10k_server_trace.log
    exit 1
fi

echo "Server PID=$SERVER_PID, public_key=$PK"
sleep 1

echo "Starting client with TRACE logging (timeout 60s)..."
timeout 60 env RUST_LOG="info,theta=trace" $CLIENT "$PK" > /tmp/c10k_client_trace.log 2>&1 || true

echo ""
echo "=== Client output ==="
cat /tmp/c10k_client_trace.log

echo ""
echo "=== Server trace log (last 100 lines) ==="
tail -100 /tmp/c10k_server_trace.log

echo ""
echo "=== Client trace summary ==="
echo "Total trace lines: $(wc -l < /tmp/c10k_client_trace.log)"
echo "C10K-DBG lines: $(grep -c 'C10K-DBG' /tmp/c10k_client_trace.log || echo 0)"
echo "open_uni mentions: $(grep -c 'open_uni\|open_stream\|starting imported' /tmp/c10k_client_trace.log || echo 0)"
echo "Error lines: $(grep -ci 'error\|fail\|timeout' /tmp/c10k_client_trace.log || echo 0)"

# Cleanup
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

echo ""
echo "Full logs: /tmp/c10k_server_trace.log and /tmp/c10k_client_trace.log"
