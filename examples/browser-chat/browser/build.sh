#!/bin/bash
set -e

# Requires: wasm-pack (cargo install wasm-pack)
# On macOS, may need: brew install llvm
# Then: export CC_wasm32_unknown_unknown="/opt/homebrew/opt/llvm/bin/clang"

cd "$(dirname "$0")"
wasm-pack build --target web --out-dir pkg

echo ""
echo "Build complete. Now build the native binary:"
echo "  cargo build -p browser-chat-native --release"
echo ""
echo "Then run:"
echo "  ./target/release/chat-peer create [port]"
echo ""
echo "Open browser at http://localhost:8080 to join via web UI"
