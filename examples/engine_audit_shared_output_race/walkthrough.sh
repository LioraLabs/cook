#!/usr/bin/env bash
# Audit walkthrough for the shared-output race fix (CS-0037).
#
# Pre-fix behavior: two non-dep-related recipes that declare the same
# `cook "<path>"` output raced silently under `--jobs > 1`. The engine
# emitted no diagnostic; the artifact's contents depended on which
# recipe's writer won the race.
#
# Post-fix behavior: the engine detects this at plan time during
# `build_dag` and reports `EngineError::OutputCollision`, naming both
# recipes and the canonical path. The CLI surfaces this as a CookError
# *before any work runs*. This script asserts the post-fix behavior.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"

# Build the cook binary if not already present.
COOK_BIN="$REPO_ROOT/cli/target/debug/cook"
if [ ! -x "$COOK_BIN" ]; then
    echo "==> building cook binary..."
    (cd "$REPO_ROOT/cli" && cargo build -p cook-cli) || {
        echo "FAIL: cargo build failed"
        exit 1
    }
fi

cd "$HERE"

# Clean any prior state.
rm -rf build .cook

echo "==> running with --jobs 4 (was previously a silent race; now MUST reject)"
OUT="$("$COOK_BIN" --jobs 4 build 2>&1)"
RC=$?

echo "----- cook output -----"
echo "$OUT"
echo "-----------------------"
echo "exit code: $RC"

# Post-fix assertions:
#   1. Non-zero exit code (the engine rejected the plan).
#   2. The diagnostic mentions "output collision".
#   3. Both colliding recipe names appear in the diagnostic.
#   4. The shared output path appears in the diagnostic.
#   5. No artifact was written (rejection happened before execution).

FAIL=0

if [ "$RC" -eq 0 ]; then
    echo "FAIL: expected non-zero exit code (collision must be rejected)"
    FAIL=1
fi

if ! echo "$OUT" | grep -qi "output collision"; then
    echo "FAIL: diagnostic missing 'output collision'"
    FAIL=1
fi

if ! echo "$OUT" | grep -q "writer_a"; then
    echo "FAIL: diagnostic missing 'writer_a'"
    FAIL=1
fi

if ! echo "$OUT" | grep -q "writer_b"; then
    echo "FAIL: diagnostic missing 'writer_b'"
    FAIL=1
fi

if ! echo "$OUT" | grep -q "shared.bin"; then
    echo "FAIL: diagnostic missing the colliding path 'shared.bin'"
    FAIL=1
fi

if [ -f "build/shared.bin" ]; then
    echo "FAIL: build/shared.bin exists — collision should be rejected before execution"
    FAIL=1
fi

if [ $FAIL -eq 0 ]; then
    echo "PASS: collision detected at plan time, both recipes named, no artifact written."
    exit 0
else
    echo "FAIL: post-fix assertions failed."
    exit 1
fi
