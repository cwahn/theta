#!/usr/bin/env python3
"""
Generate and test all possible feature combinations for the theta crate.
"""

import subprocess
import sys
from itertools import combinations

# Define all available features
FEATURES = ["macros", "tracing", "monitor", "remote", "persistence", "project_dir"]

def run_cargo_command(cmd, desc):
    """Run a cargo command and return success/failure."""
    print(f"Testing: {desc}", end=" ... ", flush=True)
    try:
        result = subprocess.run(cmd, check=True, capture_output=False, text=True)
        print("✓")
        return True
    except subprocess.CalledProcessError:
        print("✗")
        return False

def main():
    """Main test runner."""
    print("==> Testing all feature combinations for cargo check")
    
    failed_combinations = []
    total_combinations = 0
    
    # Generate all possible combinations (including empty set)
    for r in range(len(FEATURES) + 1):
        for combo in combinations(FEATURES, r):
            total_combinations += 1
            cmd = ["cargo", "check", "--no-default-features"]
            
            if combo:
                cmd.extend(["--features", ",".join(combo)])
                desc = ", ".join(combo)
            else:
                desc = "no features"
            
            if not run_cargo_command(cmd, desc):
                failed_combinations.append(combo)
    
    # Test default features
    total_combinations += 1
    if not run_cargo_command(["cargo", "check"], "default features"):
        failed_combinations.append("default")
    
    # Test all features
    total_combinations += 1
    if not run_cargo_command(["cargo", "check", "--all-features"], "all features"):
        failed_combinations.append("all-features")
    
    print(f"\n==> Check phase results: {total_combinations - len(failed_combinations)}/{total_combinations} passed")
    
    if failed_combinations:
        print("Failed combinations:")
        for combo in failed_combinations:
            print(f"  - {combo}")
        return 1
    
    print("\n==> Testing key combinations for cargo test")
    
    # For tests, only run a subset to avoid excessive test time
    test_failed = []
    test_total = 0
    
    test_combinations = [
        ([], "no features"),
        (["macros"], "macros only"),
        (["tracing"], "tracing only"),
        (["macros", "tracing"], "macros + tracing"),
        (["macros", "tracing", "monitor"], "macros + tracing + monitor"),
    ]
    
    for features, desc in test_combinations:
        test_total += 1
        cmd = ["cargo", "test", "--no-default-features"]
        if features:
            cmd.extend(["--features", ",".join(features)])
        
        if not run_cargo_command(cmd, desc):
            test_failed.append(features)
    
    # Test default features
    test_total += 1
    if not run_cargo_command(["cargo", "test"], "default features"):
        test_failed.append("default")
    
    # Test all features
    test_total += 1
    if not run_cargo_command(["cargo", "test", "--all-features"], "all features"):
        test_failed.append("all-features")
    
    print(f"\n==> Test phase results: {test_total - len(test_failed)}/{test_total} passed")
    
    if test_failed:
        print("Failed test combinations:")
        for combo in test_failed:
            print(f"  - {combo}")
        return 1
    
    print("\n==> Testing examples compilation")
    
    # Test that examples compile with their required features
    examples_failed = []
    examples_total = 0
    
    # Test individual examples to make sure they work with their required features
    examples_total += 1
    if not run_cargo_command(["cargo", "check", "--examples", "--all-features"], "all examples with all features"):
        examples_failed.append("all examples")
    
    print(f"\n==> Examples phase results: {examples_total - len(examples_failed)}/{examples_total} passed")
    
    if examples_failed:
        print("Failed examples:")
        for combo in examples_failed:
            print(f"  - {combo}")
        return 1
    
    print("\n==> All feature combination tests passed!")
    return 0

if __name__ == "__main__":
    sys.exit(main())
