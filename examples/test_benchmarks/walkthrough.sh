#!/bin/bash
# walkthrough.sh — pin the v1.0 cook --test runner against the
# test_benchmarks fixture.
#
# This stub runs cook against each recipe and prints exit codes. Phase 9
# replaces the body with full assertions on the summary block, JSON
# sidecar, --rerun-failed, and --filter behavior. Until Phase 4 lands
# `cook --test`, this script exits 0 with a "runner not yet wired" notice.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../../cli && cargo build --bin cook)"
    exit 1
fi

# Stub mode: confirm cook --test is wired, otherwise skip with notice.
if ! "$COOK" --help 2>&1 | grep -q '\-\-test'; then
    echo "[skip] cook --test not yet wired; this walkthrough is a stub"
    echo "       enabled in Phase 9 (Task 9.1) of the test-runner plan"
    exit 0
fi

echo "[walkthrough] cook --test is available; full assertions land in Phase 9"
exit 0
