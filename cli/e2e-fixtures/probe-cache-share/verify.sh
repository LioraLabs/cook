#!/usr/bin/env bash
set -euo pipefail
rm -rf build .cook
echo "--- first run (probe executes; expect >=1s) ---"
s1=$(date +%s%N); cargo run --bin cook --manifest-path ../../cli/Cargo.toml -- build; e1=$(date +%s%N)
echo "elapsed: $(( (e1 - s1) / 1000000 ))ms"

echo "--- second run (probe cached; expect <500ms) ---"
s2=$(date +%s%N); cargo run --bin cook --manifest-path ../../cli/Cargo.toml -- build; e2=$(date +%s%N)
echo "elapsed: $(( (e2 - s2) / 1000000 ))ms"

echo "timestamp: $(cat build/probed.txt) (same across both runs = cache hit)"
