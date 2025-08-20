#!/bin/sh
set -e

echo "==> Testing all feature combinations"
python3 workflow/test_features.py

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