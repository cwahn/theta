#!/bin/sh
set -e

echo "==> Testing all feature combinations"
python3 workflow/test_features.py

echo "==> Checking all workspace crates (including examples) compile"
cargo check --workspace

echo "==> Checking documentation build"
cargo doc --all-features --no-deps

echo "==> Checking version consistency across Cargo.toml and package.json files"
CARGO_VERSION=$(grep '^version' theta/Cargo.toml | head -1 | sed 's/version = "//;s/"//')
for PKG_JSON in theta-ts/ts/theta-ts/package.json theta-ts/ts/vite-plugin-theta/package.json; do
    PKG_VERSION=$(grep '"version"' "$PKG_JSON" | head -1 | sed 's/.*"version": "//;s/".*//')
    if [ "$CARGO_VERSION" != "$PKG_VERSION" ]; then
        echo "ERROR: Version mismatch! Cargo.toml=$CARGO_VERSION but $PKG_JSON=$PKG_VERSION"
        exit 1
    fi
done
echo "Version consistency OK: $CARGO_VERSION"

echo "==> Checking formatting and linting"
cargo fmt -- --check
cargo clippy --all-features -- -D warnings

echo "==> Dry-run publishing for theta-macros"
cd theta-macros
cargo publish --dry-run --allow-dirty
cd ..

# theta and theta-ts depend on the just-bumped theta-macros which isn't on crates.io yet,
# so full dry-run would fail on dep resolution. Verify package file inclusion only.
echo "==> Checking package contents for theta"
cd theta
cargo package --list --allow-dirty
cd ..

echo "==> Checking package contents for theta-ts"
cd theta-ts
cargo package --list --allow-dirty
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

echo "==> Testing web-chat WASM build"
cd examples/web-chat/react-app
npm install
npx vite build
cd ../../..

echo "==> Running example integration tests"
bash workflow/test_examples.sh

echo "==> All checks complete!"