#!/usr/bin/env bash
set -uo pipefail

# Stress test to find hanging cases
# Usage: ./examples/c1m-bench/stress_hang.sh [MAX_RUNS] [N] [TIMEOUT_SECS]

MAX_RUNS="${1:-50}"
N="${2:-1000000}"
TIMEOUT="${3:-30}"
MAX_STREAMS=$((N + 10))
FAIL_COUNT=0
PASS_COUNT=0
FAIL_LIMIT=2

BIN_DIR="./target/release"
SERVER="$BIN_DIR/c1m_server"
PROFILER="$BIN_DIR/c1m_profile"

echo "=== C1M Stress Test ==="
echo "N=$N, MAX_RUNS=$MAX_RUNS, TIMEOUT=${TIMEOUT}s, MAX_STREAMS=$MAX_STREAMS"
echo ""

for RUN in $(seq 1 "$MAX_RUNS"); do
    echo "────────────────────────────────────────"
    echo "RUN $RUN/$MAX_RUNS (pass=$PASS_COUNT fail=$FAIL_COUNT)"
    T_START=$(date +%s)

    # Start server
    MAX_STREAMS=$MAX_STREAMS RUST_LOG=error \
        "$SERVER" "$N" > /tmp/c1m_stress_server_out.txt 2>/tmp/c1m_stress_server_err.txt &
    SERVER_PID=$!

    # Wait for public key (max 25s for 1M spawn)
    PK=""
    for i in $(seq 1 50); do
        PK=$(grep -o 'PUBLIC_KEY:.*' /tmp/c1m_stress_server_out.txt 2>/dev/null | head -1 | cut -d: -f2 || true)
        if [ -n "$PK" ]; then break; fi
        sleep 0.5
    done

    if [ -z "$PK" ]; then
        T_END=$(date +%s)
        echo "  FAIL: Server didn't produce PK in 25s (elapsed $((T_END - T_START))s)"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo "  --- server stdout ---"
        cat /tmp/c1m_stress_server_out.txt 2>/dev/null
        echo "  --- server stderr (last 30) ---"
        cat /tmp/c1m_stress_server_err.txt 2>/dev/null | head -30
        kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null
        rm -f /tmp/c1m_stress_server_out.txt /tmp/c1m_stress_server_err.txt
        if [ "$FAIL_COUNT" -ge "$FAIL_LIMIT" ]; then
            echo ""; echo "REACHED $FAIL_LIMIT FAILURES — STOPPING"
            break
        fi
        continue
    fi

    echo "  Server ready (PK=${PK:0:12}...)"

    # Run profiler with timeout
    MAX_STREAMS=$MAX_STREAMS RUST_LOG=error,theta::remote::peer=error \
        timeout "$TIMEOUT" "$PROFILER" "$PK" \
        > /tmp/c1m_stress_client_out.txt 2>/tmp/c1m_stress_client_err.txt
    CLIENT_EXIT=$?

    T_END=$(date +%s)
    ELAPSED=$((T_END - T_START))

    if [ "$CLIENT_EXIT" -eq 0 ]; then
        # Extract key metrics
        SETUP_LINE=$(grep 'setup OK' /tmp/c1m_stress_client_out.txt 2>/dev/null | tail -1)
        PASS_COUNT=$((PASS_COUNT + 1))
        echo "  PASS (${ELAPSED}s) $SETUP_LINE"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo "  FAIL (exit=$CLIENT_EXIT, ${ELAPSED}s) — likely HUNG/TIMEOUT"

        # Capture diagnostic info
        echo "  --- client stdout (last 20) ---"
        cat /tmp/c1m_stress_client_out.txt 2>/dev/null | head -40
        echo "  --- client stderr (last 20) ---"
        cat /tmp/c1m_stress_client_err.txt 2>/dev/null | head -30
        echo "  --- server stderr (last 20) ---"
        cat /tmp/c1m_stress_server_err.txt 2>/dev/null | head -30

        # Save failure artifacts
        cp /tmp/c1m_stress_client_out.txt "/tmp/c1m_fail_${FAIL_COUNT}_client_out.txt" 2>/dev/null
        cp /tmp/c1m_stress_client_err.txt "/tmp/c1m_fail_${FAIL_COUNT}_client_err.txt" 2>/dev/null
        cp /tmp/c1m_stress_server_err.txt "/tmp/c1m_fail_${FAIL_COUNT}_server_err.txt" 2>/dev/null
        cp /tmp/c1m_stress_server_out.txt "/tmp/c1m_fail_${FAIL_COUNT}_server_out.txt" 2>/dev/null
    fi

    # Kill server
    kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null
    rm -f /tmp/c1m_stress_server_out.txt /tmp/c1m_stress_server_err.txt
    rm -f /tmp/c1m_stress_client_out.txt /tmp/c1m_stress_client_err.txt

    if [ "$FAIL_COUNT" -ge "$FAIL_LIMIT" ]; then
        echo ""; echo "REACHED $FAIL_LIMIT FAILURES — STOPPING"
        break
    fi

    sleep 1
done

echo ""
echo "=== SUMMARY ==="
echo "Runs: $((PASS_COUNT + FAIL_COUNT)), Pass: $PASS_COUNT, Fail: $FAIL_COUNT"
if [ "$FAIL_COUNT" -gt 0 ]; then
    echo "Failure artifacts saved at /tmp/c1m_fail_*"
fi
