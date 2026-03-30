#!/bin/bash
#
# Integration tests for theta examples.
# Builds once, then runs pre-built binaries directly.
# All networked processes are backgrounded and killed the moment a result is seen.
#
# Usage:  ./workflow/test_examples.sh
#
set -uo pipefail

# If a previous run killed client without restoring the terminal, raw mode
# may still be active. Reset to sane defaults before producing any output.
stty sane 2>/dev/null || true

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

PASS=0
FAIL=0
SKIP=0
PIDS=()
TMPDIR=$(mktemp -d)
BIN=./target/debug

cleanup() {
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
    done
    rm -rf "$TMPDIR"
    # Restore terminal in case any process (e.g. client via crossterm) left raw mode on.
    stty sane 2>/dev/null || true
}
trap cleanup EXIT

track_pid() { PIDS+=("$1"); }
ts()        { date '+[%H:%M:%S]'; }

pass()   { echo -e "$(ts) ${GREEN}PASS${NC}  $1"; ((PASS++)); }
fail()   { echo -e "$(ts) ${RED}FAIL${NC}  $1 -- $2"; ((FAIL++)); }
skip()   { echo -e "$(ts) ${YELLOW}SKIP${NC}  $1 -- $2"; ((SKIP++)); }
header() { echo -e "\n$(ts) ${BOLD}[$1]${NC}"; }

show_log() {
    for f in "$@"; do
        [ -f "$f" ] || continue
        echo "    -- $(basename "$f") --"
        tail -20 "$f" | sed 's/^/    /'
    done
}

extract_key() {
    grep -oE '[0-9a-f]{64}' "$1" 2>/dev/null | head -1
}

# Poll one file for a pattern within timeout seconds.
wait_for() {
    local file=$1 pattern=$2 timeout=${3:-5}
    local deadline=$((SECONDS + timeout))
    while ! grep -q "$pattern" "$file" 2>/dev/null; do
        [ $SECONDS -ge $deadline ] && return 1
        sleep 0.05
    done
}

# Poll two files -- succeeds when BOTH match.
wait_for_both() {
    local f1=$1 f2=$2 pattern=$3 timeout=${4:-5}
    local deadline=$((SECONDS + timeout))
    while true; do
        grep -q "$pattern" "$f1" 2>/dev/null \
            && grep -q "$pattern" "$f2" 2>/dev/null \
            && return 0
        [ $SECONDS -ge $deadline ] && return 1
        sleep 0.05
    done
}

# Poll two files -- succeeds when EITHER matches.
wait_for_either() {
    local f1=$1 f2=$2 pattern=$3 timeout=${4:-10}
    local deadline=$((SECONDS + timeout))
    while true; do
        grep -q "$pattern" "$f1" 2>/dev/null && return 0
        grep -q "$pattern" "$f2" 2>/dev/null && return 0
        [ $SECONDS -ge $deadline ] && return 1
        sleep 0.05
    done
}

# Open a FIFO with O_RDWR (<>) so the open() call never blocks.
# O_WRONLY (>) blocks until a reader opens the other end; since the reader
# process has not started yet, that would deadlock the script permanently.
# Sets global FIFO_FD to the allocated fd number.
open_fifo() {
    local fifo=$1
    mkfifo "$fifo"
    exec {FIFO_FD}<>"$fifo"
}

# -- Build ---------------------------------------------------------------------
header "Build"
if ! cargo build --workspace 2>&1 | tail -3; then
    echo -e "${RED}Build failed${NC}"
    exit 1
fi

# -- 1. basics/readme_counter -------------------------------------------------
header "basics/readme_counter"
RC_LOG="$TMPDIR/readme_counter.log"
$BIN/readme_counter >"$RC_LOG" 2>&1
if grep -q "Current value: 5" "$RC_LOG"; then
    pass "readme_counter prints correct value"
    rm -f "$RC_LOG"
else
    fail "readme_counter" "expected 'Current value: 5'"
    show_log "$RC_LOG"
fi

# -- 2. basics/tell_error -----------------------------------------------------
header "basics/tell_error"
TE_LOG="$TMPDIR/tell_error.log"
if $BIN/tell_error >"$TE_LOG" 2>&1; then
    pass "tell_error exits successfully"
    rm -f "$TE_LOG"
else
    fail "tell_error" "non-zero exit code"
    show_log "$TE_LOG"
fi

# -- 3. counter/host + counter/client -----------------------------------------
header "counter (host + client)"
# host prints key via println! (stdout). client reads key from stdin then
# enters a crossterm interactive loop -- background it and kill on success.
HOST_LOG="$TMPDIR/host.log"
CLIENT_LOG="$TMPDIR/client.log"

$BIN/host >"$HOST_LOG" 2>&1 &
HOST_PID=$!; track_pid $HOST_PID

if wait_for "$HOST_LOG" "public key:" 5; then
    HOST_KEY=$(extract_key "$HOST_LOG")
    if [ -z "$HOST_KEY" ]; then
        fail "counter" "could not extract host public key"
        show_log "$HOST_LOG"
    else
        printf '%s\n' "$HOST_KEY" >"$TMPDIR/host.key"
        $BIN/client <"$TMPDIR/host.key" >"$CLIENT_LOG" 2>&1 &
        CLIENT_PID=$!; track_pid $CLIENT_PID

        if wait_for "$CLIENT_LOG" "worker actor IDs match" 10; then
            pass "client connected and verified worker IDs"
            rm -f "$HOST_LOG" "$CLIENT_LOG"
        else
            fail "counter/client" "did not match worker actor IDs"
            show_log "$HOST_LOG" "$CLIENT_LOG"
        fi
        kill $CLIENT_PID 2>/dev/null; wait $CLIENT_PID 2>/dev/null
        # client uses crossterm enable_raw_mode; restore terminal after killing it.
        stty sane 2>/dev/null || true
    fi
else
    fail "counter/host" "host did not start in time"
    show_log "$HOST_LOG"
fi
kill $HOST_PID 2>/dev/null; wait $HOST_PID 2>/dev/null

# -- 4. monitoring/monitored + monitoring/monitor -----------------------------
header "monitoring (monitored + monitor)"
# monitored logs key via tracing info! (stderr). monitor reads key from stdin
# then loops receiving state updates -- background it and kill on success.
MONITORED_LOG="$TMPDIR/monitored.log"
MONITOR_LOG="$TMPDIR/monitor.log"

$BIN/monitored >"$MONITORED_LOG" 2>&1 &
MONITORED_PID=$!; track_pid $MONITORED_PID

if wait_for "$MONITORED_LOG" "public key:" 5; then
    MONITORED_KEY=$(extract_key "$MONITORED_LOG")
    if [ -z "$MONITORED_KEY" ]; then
        fail "monitoring" "could not extract monitored public key"
        show_log "$MONITORED_LOG"
    else
        printf '%s\n' "$MONITORED_KEY" >"$TMPDIR/monitored.key"
        $BIN/monitor <"$TMPDIR/monitored.key" >"$MONITOR_LOG" 2>&1 &
        MONITOR_PID=$!; track_pid $MONITOR_PID

        if wait_for "$MONITOR_LOG" "received state update" 10; then
            pass "monitor received state updates"
            rm -f "$MONITORED_LOG" "$MONITOR_LOG"
        else
            fail "monitoring/monitor" "no state updates received"
            show_log "$MONITORED_LOG" "$MONITOR_LOG"
        fi
        kill $MONITOR_PID 2>/dev/null; wait $MONITOR_PID 2>/dev/null
    fi
else
    fail "monitoring/monitored" "monitored did not start in time"
    show_log "$MONITORED_LOG"
fi
kill $MONITORED_PID 2>/dev/null; wait $MONITORED_PID 2>/dev/null

# -- 5. ping-pong/peer --------------------------------------------------------
header "ping-pong (peer x 2)"
#
# FIFOs are opened with <> (O_RDWR) in the PARENT shell -- this never blocks.
# Using > (O_WRONLY) blocks until a reader opens the other end; since the peer
# process has not started yet, that would deadlock the script permanently.
#
PA_LOG="$TMPDIR/peer_a.log"
PB_LOG="$TMPDIR/peer_b.log"
PA_FIFO="$TMPDIR/pa.fifo"
PB_FIFO="$TMPDIR/pb.fifo"

open_fifo "$PA_FIFO"; PA_FD=$FIFO_FD
open_fifo "$PB_FIFO"; PB_FD=$FIFO_FD

$BIN/peer <"$PA_FIFO" >"$PA_LOG" 2>&1 &
PA_PID=$!; track_pid $PA_PID

$BIN/peer <"$PB_FIFO" >"$PB_LOG" 2>&1 &
PB_PID=$!; track_pid $PB_PID

if wait_for_both "$PA_LOG" "$PB_LOG" "public key:" 5; then
    KEY_B=$(extract_key "$PB_LOG")
    if [ -z "$KEY_B" ]; then
        fail "ping-pong" "could not extract peer B public key"
        show_log "$PA_LOG" "$PB_LOG"
    else
        # Only send peer B's key to peer A -- one-directional avoids the race
        # where both peers simultaneously call lookup() and iroh drops one.
        printf '%s\n' "$KEY_B" >&$PA_FD
        if wait_for "$PA_LOG" "received pong" 30; then
            pass "peer A sent ping and received pong from peer B"
            rm -f "$PA_LOG" "$PB_LOG"
        else
            fail "ping-pong" "no pong received within 30s"
            show_log "$PA_LOG" "$PB_LOG"
        fi
    fi
else
    fail "ping-pong" "peers did not both start in time"
    show_log "$PA_LOG" "$PB_LOG"
fi

exec {PA_FD}>&-; exec {PB_FD}>&-
kill $PA_PID $PB_PID 2>/dev/null; wait $PA_PID $PB_PID 2>/dev/null

# -- 6. ping-pong/forward -----------------------------------------------------
header "ping-pong (forward x 2)"
FA_LOG="$TMPDIR/fwd_a.log"
FB_LOG="$TMPDIR/fwd_b.log"
FA_FIFO="$TMPDIR/fa.fifo"
FB_FIFO="$TMPDIR/fb.fifo"

open_fifo "$FA_FIFO"; FA_FD=$FIFO_FD
open_fifo "$FB_FIFO"; FB_FD=$FIFO_FD

$BIN/forward <"$FA_FIFO" >"$FA_LOG" 2>&1 &
FA_PID=$!; track_pid $FA_PID

$BIN/forward <"$FB_FIFO" >"$FB_LOG" 2>&1 &
FB_PID=$!; track_pid $FB_PID

if wait_for_both "$FA_LOG" "$FB_LOG" "public key:" 5; then
    KEY_FB=$(extract_key "$FB_LOG")
    if [ -z "$KEY_FB" ]; then
        fail "forward" "could not extract forward peer B public key"
        show_log "$FA_LOG" "$FB_LOG"
    else
        # Only send peer B's key to peer A -- one-directional avoids the race
        # where both peers simultaneously call lookup() and iroh drops one.
        printf '%s\n' "$KEY_FB" >&$FA_FD
        if wait_for "$FB_LOG" "received forwarded ping" 30; then
            pass "forward peer A pinged B; B logged received forwarded ping"
            rm -f "$FA_LOG" "$FB_LOG"
        else
            fail "forward" "no forwarded ping received within 30s"
            show_log "$FA_LOG" "$FB_LOG"
        fi
    fi
else
    fail "forward" "forward peers did not both start in time"
    show_log "$FA_LOG" "$FB_LOG"
fi

exec {FA_FD}>&-; exec {FB_FD}>&-
kill $FA_PID $FB_PID 2>/dev/null; wait $FA_PID $FB_PID 2>/dev/null

# -- 7. dedup -----------------------------------------------------------------
header "dedup"
DEDUP_LOG="$TMPDIR/dedup.log"

$BIN/dedup >"$DEDUP_LOG" 2>&1 &
DEDUP_PID=$!; track_pid $DEDUP_PID

if wait_for "$DEDUP_LOG" "received pong" 20; then
    pass "dedup connection established and ping/pong succeeded"
    rm -f "$DEDUP_LOG"
else
    fail "dedup" "no pong received within 20s"
    show_log "$DEDUP_LOG"
fi
kill $DEDUP_PID 2>/dev/null; wait $DEDUP_PID 2>/dev/null

# -- 8. bench (skip) ----------------------------------------------------------
header "ping-pong/bench"
skip "bench" "performance benchmark, not a functional test"

# -- Summary ------------------------------------------------------------------
echo ""
echo -e "$(ts) ${BOLD}Results${NC}"
echo -e "  ${GREEN}Passed: $PASS${NC}"
[ $FAIL -gt 0 ] && echo -e "  ${RED}Failed: $FAIL${NC}" || echo "  Failed: 0"
[ $SKIP -gt 0 ] && echo -e "  ${YELLOW}Skipped: $SKIP${NC}"
echo ""

if [ $FAIL -gt 0 ]; then
    echo -e "${RED}Some tests failed.${NC}"
    exit 1
else
    echo -e "${GREEN}All tests passed!${NC}"
fi
