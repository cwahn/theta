#!/bin/bash
# Run multiple tests with fresh server each time, capturing stderr for each
NUM_RUNS="${1:-10}"
TIMEOUT="${2:-35}"
WORKERS="${3:-100000}"

mkdir -p /tmp/c1m_tests

echo "=== Running $NUM_RUNS tests with $WORKERS workers, timeout=${TIMEOUT}s ==="
echo "=== Fresh server started for each run ==="
echo ""

pass=0
fail=0

for i in $(seq 1 "$NUM_RUNS"); do
    SERVER_STDERR="/tmp/c1m_tests/run_${i}_server_stderr.txt"
    CLIENT_STDERR="/tmp/c1m_tests/run_${i}_client_stderr.txt"
    STDOUT_FILE="/tmp/c1m_tests/run_${i}_stdout.txt"
    
    # Kill any lingering server
    pkill -f "c10k_server" 2>/dev/null
    sleep 1
    
    # Start fresh server, capture its stderr
    MAX_STREAMS=$((WORKERS * 2)) RUST_LOG=error \
        ./target/release/c10k_server "$WORKERS" \
        > /tmp/c1m_tests/run_${i}_server_stdout.txt 2> "$SERVER_STDERR" &
    SERVER_PID=$!
    
    # Wait for server to print PUBLIC_KEY
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
    
    # Kill server
    kill -INT $SERVER_PID 2>/dev/null
    wait $SERVER_PID 2>/dev/null
    
    # Analyze results
    local_gt=$(grep "local > server" "$CLIENT_STDERR" 2>/dev/null | head -1 | sed 's/\[CLIENT\] //')
    
    if [ $exit_code -eq 124 ]; then
        echo "TIMEOUT after ${elapsed}s"
        fail=$((fail + 1))
        last_phase=$(grep -oE '\[[0-9]+[ab]?\.' "$STDOUT_FILE" | tail -1)
        echo "  Last phase: $last_phase"
        setup_line=$(grep "setup OK" "$STDOUT_FILE" | tail -1)
        echo "  Setup: $setup_line"
        echo "  $local_gt"
        # Server-side accept count
        svr_accept=$(grep "accept_bi FAILED\|accepted.*bi-streams" "$SERVER_STDERR" 2>/dev/null | tail -1)
        [ -n "$svr_accept" ] && echo "  Server: $svr_accept"
        # Client open_bi failures
        ob_fail=$(grep -c "open_bi FAILED" "$CLIENT_STDERR" 2>/dev/null)
        echo "  Client open_bi failures: $ob_fail"
        # DEDUP
        dedup_cli=$(grep -c "DEDUP\|UNFAVORED" "$CLIENT_STDERR" 2>/dev/null)
        dedup_svr=$(grep -c "DEDUP\|UNFAVORED" "$SERVER_STDERR" 2>/dev/null)
        echo "  DEDUP msgs: client=$dedup_cli server=$dedup_svr"
        # Control frames
        ctrl_count=$(grep -c "CTRL.*LookupReq\|CTRL.*LookupResp" "$SERVER_STDERR" 2>/dev/null)
        echo "  Server CTRL frames: $ctrl_count"
    elif [ $exit_code -eq 0 ]; then
        echo "PASS in ${elapsed}s"
        pass=$((pass + 1))
        setup_line=$(grep "setup OK" "$STDOUT_FILE" | tail -1)
        echo "  $setup_line"
        echo "  $local_gt"
    else
        echo "ERROR(exit=$exit_code) after ${elapsed}s"
        fail=$((fail + 1))
        err_line=$(tail -5 "$CLIENT_STDERR" | head -3)
        echo "  $err_line"
        echo "  $local_gt"
        # Server state
        svr_accept=$(grep "accept_bi FAILED\|accepted.*bi-streams" "$SERVER_STDERR" 2>/dev/null | tail -1)
        [ -n "$svr_accept" ] && echo "  Server: $svr_accept"
    fi
    
    sleep 1
done

echo ""
echo "=== Results: $pass PASS, $fail FAIL out of $NUM_RUNS runs ==="
