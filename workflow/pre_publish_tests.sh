#!/bin/sh
set -e

echo "==> Testing all feature combinations"
python3 workflow/test_features.py

echo "==> Checking all workspace crates (including examples) compile"
cargo check --workspace

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

echo "==> Building and checking TypeScript packages"
cd theta-ts
pnpm install --frozen-lockfile
pnpm -r build
pnpm -r typecheck
cd ..

echo "==> Dry-run publishing for npm packages"
cd theta-ts/ts/theta-ts
npm publish --dry-run --tag alpha
cd ../../..
cd theta-ts/ts/vite-plugin-theta
npm publish --dry-run --tag alpha
cd ../../..

echo "==> Running example integration tests"
bash workflow/test_examples.sh

echo "==> All checks complete!"