#!/usr/bin/env python3
"""Run two ping_pong_bench instances with logging, exchange keys, capture output."""

import subprocess
import threading
import time
import sys
import re
import os

BINARY = "target/release/examples/ping_pong_bench"
# Log iroh relay/connection activity and theta remote peer activity
RUST_LOG = "warn,iroh::socket::remote_map=debug,iroh::endpoint=debug,iroh::socket=info,theta::remote=debug"

def reader(proc, name, lines, stream, prefix=""):
    """Read a stream line by line, printing and collecting."""
    for raw in iter(stream.readline, b''):
        line = raw.decode('utf-8', errors='replace').rstrip('\n')
        lines.append(line)
        tag = f"[{name}{prefix}]"
        print(f"{tag} {line}", flush=True)

def extract_public_key(lines, timeout=60):
    start = time.time()
    pk_pattern = re.compile(r'public key:\s*(\S+)')
    while time.time() - start < timeout:
        for line in lines:
            m = pk_pattern.search(line)
            if m:
                return m.group(1)
        time.sleep(0.1)
    return None

def wait_for_prompt(lines, timeout=60):
    start = time.time()
    while time.time() - start < timeout:
        for line in lines:
            if 'Please enter' in line:
                return True
        time.sleep(0.1)
    return False

def wait_for_done(lines, timeout=300):
    """Wait for benchmark to finish (either results or error)."""
    start = time.time()
    while time.time() - start < timeout:
        for line in lines:
            if 'THROUGHPUT ANALYSIS' in line or 'Benchmark error' in line:
                return True
        time.sleep(0.5)
    return False

def main():
    env = os.environ.copy()
    env["RUST_LOG"] = RUST_LOG

    print(f"=== Starting benchmark with RUST_LOG={RUST_LOG} ===", flush=True)
    
    proc_a = subprocess.Popen(
        [BINARY],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=env,
    )
    proc_b = subprocess.Popen(
        [BINARY],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=env,
    )

    stdout_a, stderr_a = [], []
    stdout_b, stderr_b = [], []

    for proc, name, out_lines, err_lines in [
        (proc_a, 'A', stdout_a, stderr_a),
        (proc_b, 'B', stdout_b, stderr_b),
    ]:
        threading.Thread(target=reader, args=(proc, name, out_lines, proc.stdout), daemon=True).start()
        threading.Thread(target=reader, args=(proc, name, err_lines, proc.stderr, " LOG"), daemon=True).start()

    # Wait for public keys
    pk_a = extract_public_key(stdout_a)
    pk_b = extract_public_key(stdout_b)
    if not pk_a or not pk_b:
        print("FAILED to get public keys", flush=True)
        proc_a.kill(); proc_b.kill()
        return 1

    print(f"\n=== Keys: A={pk_a[:16]}... B={pk_b[:16]}... ===", flush=True)
    
    if not wait_for_prompt(stdout_a) or not wait_for_prompt(stdout_b):
        print("FAILED to get prompt", flush=True)
        proc_a.kill(); proc_b.kill()
        return 1
    
    print("=== Exchanging keys ===", flush=True)
    proc_a.stdin.write(f"{pk_b}\n".encode()); proc_a.stdin.flush()
    proc_b.stdin.write(f"{pk_a}\n".encode()); proc_b.stdin.flush()
    
    # Wait for both to finish
    a_done = threading.Event()
    b_done = threading.Event()
    def w_a():
        if wait_for_done(stdout_a): a_done.set()
    def w_b():
        if wait_for_done(stdout_b): b_done.set()
    threading.Thread(target=w_a, daemon=True).start()
    threading.Thread(target=w_b, daemon=True).start()
    
    a_done.wait(timeout=300)
    b_done.wait(timeout=300)
    
    if not a_done.is_set():
        print("[A] TIMEOUT", flush=True)
    if not b_done.is_set():
        print("[B] TIMEOUT", flush=True)
    
    time.sleep(2)
    proc_a.kill(); proc_b.kill()
    proc_a.wait(); proc_b.wait()
    
    # Summary: count log lines with relay/direct info
    print("\n=== LOG SUMMARY ===", flush=True)
    for name, err_lines in [('A', stderr_a), ('B', stderr_b)]:
        relay_count = sum(1 for l in err_lines if 'relay' in l.lower() or 'Relay' in l)
        direct_count = sum(1 for l in err_lines if 'direct' in l.lower() or 'holepunch' in l.lower())
        total = len(err_lines)
        print(f"[{name}] total_log_lines={total}, relay_mentions={relay_count}, direct/holepunch_mentions={direct_count}", flush=True)
    
    print("\n=== Done ===", flush=True)
    return 0

if __name__ == '__main__':
    sys.exit(main())
