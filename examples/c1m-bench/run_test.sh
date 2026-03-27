#!/bin/bash
# Run multiple tests with fresh server each time
# Usage: ./examples/c1m-bench/run_test.sh [NUM_RUNS] [TIMEOUT] [WORKERS]
NUM_RUNS="${1:-10}"
TIMEOUT="${2:-120}"
WORKERS="${3:-100000}"

mkdir -p /tmp/c1m_tests

echo "=== Running $NUM_RUNS tests with $WORKERS workers, timeout=${TIMEOUT}s ==="
echo ""

pass=0
fail=0

for i in $(seq 1 "$NUM_RUNS"); do
    SERVER_STDERR="/tmp/c1m_tests/run_${i}_server_stderr.txt"
    CLIENT_STDERR="/tmp/c1m_tests/run_${i}_client_stderr.txt"
    STDOUT_FILE="/tmp/c1m_tests/run_${i}_stdout.txt"

    pkill -f "c1m_server" 2>/dev/null
    sleep 1

    MAX_STREAMS=$((WORKERS * 2)) RUST_LOG=error \
        ./target/release/c1m_server "$WORKERS" \
        > /tmp/c1m_tests/run_${i}_server_stdout.txt 2> "$SERVER_STDERR" &
    SERVER_PID=$!

    SERVER_KEY=""
    for w in $(seq 1 10); do
        sleep 1
        SERVER_KEY=$(grep "^PUBLIC_KEY:" /tmp/c1m_tests/run_${i}_server_stdout.txt 2>/dev/null | sed 's/PUBLIC_KEY://')
        [ -n "$SERVER_KEY" ] && break
    done

    if [ -z "$SERVER_KEY" ]; then
        echo "Run $i/$NUM_RUNS: FAILED to start server"
        fail=$((fail + 1))
        kill $SERVER_PID 2>/dev/null
        continue
    fi

    echo -n "Run $i/$NUM_RUNS: "
    start_time=$(date +%s)

    timeout "$TIMEOUT" env MAX_STREAMS=$((WORKERS * 2)) RUST_LOG=error \
        ./target/release/c1m_profile "$SERVER_KEY" \
        > "$STDOUT_FILE" 2> "$CLIENT_STDERR"

    exit_code=$?
    end_time=$(date +%s)
    elapsed=$((end_time - start_time))

    kill -INT $SERVER_PID 2>/dev/null
    wait $SERVER_PID 2>/dev/null

    if [ $exit_code -eq 124 ]; then
        echo "TIMEOUT after ${elapsed}s"
        fail=$((fail + 1))
        last_phase=$(grep -oE '\[[0-9]+[ab]?\.' "$STDOUT_FILE" | tail -1)
        echo "  Last phase: $last_phase"
        setup_line=$(grep "setup OK" "$STDOUT_FILE" | tail -1)
        echo "  Setup: $setup_line"
    elif [ $exit_code -eq 0 ]; then
        echo "PASS in ${elapsed}s"
        pass=$((pass + 1))
        setup_line=$(grep "setup OK" "$STDOUT_FILE" | tail -1)
        echo "  $setup_line"
    else
        echo "ERROR(exit=$exit_code) after ${elapsed}s"
        fail=$((fail + 1))
        tail -3 "$CLIENT_STDERR"
    fi

    sleep 1
done

echo ""
echo "=== Results: $pass PASS, $fail FAIL out of $NUM_RUNS runs ==="
