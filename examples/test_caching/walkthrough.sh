#!/bin/bash
# walkthrough.sh — pin v1.0 test-result caching contract. Stub until Phase 9.

set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    exit 1
fi

if ! "$COOK" --help 2>&1 | grep -q '\-\-test'; then
    echo "[skip] cook --test not yet wired; assertions land in Phase 9.3"
    exit 0
fi

echo "[walkthrough] cook --test available; cache assertions enabled in Phase 9.3"
exit 0
