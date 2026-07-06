#!/bin/sh
set -e
mkdir -p out
echo "core $1" > out/core.js
echo "wasm $1" > out/core_bg.wasm
echo "types $1" > out/core.d.ts
