#!/bin/sh
set -e

echo "==> Checking all major feature combinations for theta"
cargo check --features "macros"
cargo check --features "macros,monitor"
cargo check --features "macros,remote"
cargo check --features "macros,persistence"
cargo check --features "macros,remote,monitor"
cargo check --all-features

echo "==> Running tests for all major feature combinations"
cargo test --features "macros"
cargo test --features "macros,monitor"
cargo test --features "macros,remote"
cargo test --features "macros,persistence"
cargo test --features "macros,remote,monitor"
cargo test --all-features

echo "==> Checking documentation build"
cargo doc --all-features --no-deps

echo "==> Checking formatting and linting"
cargo fmt -- --check
cargo clippy --all-features -- -D warnings

echo "==> Dry-run publishing for theta"
cd theta
cargo publish --dry-run --allow-dirty
cd ..

echo "==> Dry-run publishing for theta-macros"
cd theta-macros
cargo publish --dry-run --allow-dirty
cd ..

echo "==> All checks complete!"