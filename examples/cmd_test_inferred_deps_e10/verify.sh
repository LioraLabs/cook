#!/bin/bash
# verify.sh — pin App. E.10 (inferred_deps asymmetry across CLI commands).
#
# Pre-fix: `cook --test` passes &BTreeMap::new() to the wave grouper. Any
# Cookfile whose test body references another recipe via {NAME} fails at
# registration with "recipe '...' has no terminal output".
#
# Post-fix: cmd_test computes inferred_deps via compute_single_inferred_deps
# (single-Cookfile path) or compute_workspace_inferred_deps (workspace path),
# matching cmd_run.

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

clean_state() {
    rm -rf .cook build 2>/dev/null || true
}

echo "=== Appendix E.10 verification ==="
echo

clean_state
test_out=$("$COOK" --test --jobs 1 2>&1)

scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "cook --test does not error at registration"
if echo "$test_out" | grep -q "no terminal output"; then
    echo "FAIL"
    echo "      $(echo "$test_out" | grep "no terminal output" | head -1)"
    fail=$((fail + 1))
else
    echo "PASS"
    pass=$((pass + 1))
fi

scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "verify_prepare runs after prepare in the same wave"
if echo "$test_out" | grep -qE "verify_prepare +done"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "      got: $(echo "$test_out" | tail -3)"
    fail=$((fail + 1))
fi

clean_state

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
