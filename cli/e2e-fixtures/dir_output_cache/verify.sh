#!/bin/bash
# verify.sh — end-to-end smoke for CS-0115 directory outputs.
#
# Asserts:
#   1. Cold build produces a directory output (out/).
#   2. Cache hit restores the full directory tree.
#   3. Build-owned: a stray file inside the output dir is swept on cache hit.
#   4. cook cache verify round-trips a directory output correctly.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../../cli && cargo build --bin cook)"
    exit 1
fi

rm -rf .cook out

"$COOK" gen                                  # cold build
test -f out/core.js && test -f out/core_bg.wasm && test -f out/core.d.ts

rm -rf out
"$COOK" gen                                  # cache hit restores the whole tree
test -f out/core.js && test -f out/core_bg.wasm && test -f out/core.d.ts

echo stray > out/STRAY.txt
"$COOK" gen                                  # build-owned: stray swept on cache hit
test ! -e out/STRAY.txt
test -f out/core.js                          # real outputs intact

"$COOK" cache verify gen                     # dir output round-trips under verify
echo "OK dir_output_cache"
