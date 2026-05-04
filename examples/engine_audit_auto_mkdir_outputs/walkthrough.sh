#!/usr/bin/env bash
# Audit walkthrough for CS-0050: the engine MUST create the parent
# directory of every declared cook-step output before invoking the
# step's shell text.
#
# Pre-fix behavior: the recipe `cook "build/out/foo.txt" using { echo hi > $<out> }`
# (no `mkdir -p` in the body) failed with "build/out/foo.txt: No such
# file or directory" because `build/out/` did not exist when the
# subprocess ran.
#
# Post-fix behavior: the engine ensures the parent directory exists
# before dispatching the shell text. The recipe runs cleanly and the
# declared output is produced. This script asserts the post-fix
# behavior end-to-end.

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

# Clean any prior state — the test relies on `build/` not existing.
rm -rf build .cook

# Sanity check: the recipe text MUST NOT contain `mkdir -p` outside
# of comment lines. (If a future commit re-introduces the boilerplate
# inside the cook-step body, this fixture's claim would be vacuous.)
if grep -v '^[[:space:]]*#' Cookfile | grep -q 'mkdir -p'; then
    echo "FAIL: Cookfile contains 'mkdir -p' in non-comment lines — fixture is supposed to test the pre-step engine mkdir."
    exit 1
fi

echo "==> running 'cook build' (no mkdir -p in the recipe; engine MUST create parent dir)"
OUT="$("$COOK_BIN" build 2>&1)"
RC=$?

echo "----- cook output -----"
echo "$OUT"
echo "-----------------------"
echo "exit code: $RC"

FAIL=0

if [ "$RC" -ne 0 ]; then
    echo "FAIL: expected exit code 0, got $RC"
    FAIL=1
fi

if [ ! -f "build/out/foo.txt" ]; then
    echo "FAIL: build/out/foo.txt does not exist (engine should have created its parent dir)"
    FAIL=1
fi

if [ -f "build/out/foo.txt" ]; then
    BODY="$(cat build/out/foo.txt)"
    if [ "$BODY" != "hi" ]; then
        echo "FAIL: build/out/foo.txt content was '$BODY', expected 'hi'"
        FAIL=1
    fi
fi

# Cleanup.
rm -rf build .cook

if [ $FAIL -eq 0 ]; then
    echo "PASS: engine created parent directories of declared cook outputs (CS-0050)."
    exit 0
else
    echo "FAIL: post-fix assertions failed."
    exit 1
fi
