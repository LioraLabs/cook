#!/bin/bash
# walkthrough.sh — pin the CS-0035 multi-line tool-fingerprint contract
# end-to-end through the cook binary.
#
# Scenario:
#   1. Install variant A of build_tool.sh, build → all execute.
#   2. Re-build with no changes → all hit (sanity).
#   3. Swap in variant B (different bytes ⇒ different sha256), no source
#      change, no command-text change → MUST rebuild because tool content
#      changed and is now part of the context hash.
#
# Pre-CS-0035 the third assertion failed: the cache reported a hit because
# only the first line's first token (`mkdir`) was fingerprinted, and that
# binary is unchanged.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../../cli && cargo build --bin cook)"
    exit 1
fi

pass=0
fail=0
scenario=0

run_assert() {
    local name="$1"
    local expected="$2"
    shift 2
    scenario=$((scenario + 1))
    printf "  [%2d] %-55s " "$scenario" "$name"
    local out
    out="$("$@" 2>&1 | grep -oE 'cook build done.*\)' | head -1)"
    if [[ "$out" == *"$expected"* ]]; then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL"
        echo "       expected substring: $expected"
        echo "       got:                $out"
        fail=$((fail + 1))
    fi
}

clean_state() {
    rm -rf build .cook 2>/dev/null || true
    rm -f build_tool.sh
}

echo "=== CS-0035 cache_audit_first_token verification ==="
echo

clean_state

# Install variant A.
cp tools_a/build_tool.sh build_tool.sh
chmod +x build_tool.sh

run_assert "Fresh build under tool A executes 1 recipe" "0 cached recipes, 1 done" "$COOK" build
run_assert "Re-build under tool A hits"                 "1 cached recipes, 1 done" "$COOK" build

# Swap in variant B. No Cookfile change, no input-source change.
cp tools_b/build_tool.sh build_tool.sh
chmod +x build_tool.sh

run_assert "Tool swap (A→B) MUST invalidate cache"      "0 cached recipes, 1 done" "$COOK" build
run_assert "Re-build under tool B hits"                 "1 cached recipes, 1 done" "$COOK" build

# Swap back to variant A. The fingerprint matches the entry recorded in
# scenario 1, so cook restores from the local cache (no re-execution).
# This proves the fingerprint is a function of tool *content*, not a
# monotonic version counter — round-tripping is safe and cheap.
cp tools_a/build_tool.sh build_tool.sh
chmod +x build_tool.sh

run_assert "Tool swap (B→A) restores prior A entry"     "1 cached recipes, 1 done" "$COOK" build

clean_state

echo
echo "=== summary: $pass passed, $fail failed ==="
[ "$fail" -eq 0 ]
