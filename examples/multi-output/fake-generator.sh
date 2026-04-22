#!/bin/sh
# Simulates wasm-pack: reads src/*, writes two declared outputs to pkg/.
set -eu
mkdir -p pkg
echo "// generated from $(find src -type f | sort)" > pkg/out.js
echo "BINARY" > pkg/out.wasm
