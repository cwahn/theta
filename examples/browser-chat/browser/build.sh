#!/bin/bash
set -e
wasm-pack build --target web --out-dir pkg
echo "Build complete. Serve this directory with a local HTTP server:"
echo "  python3 -m http.server 8080"
