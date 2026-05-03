#!/bin/bash
# Bug-hunt fixture: test recipes referencing imported recipes via {alias.recipe}.
# CS-0027 calls out: add_test doesn't propagate step_group_dep_refs into
# unit-DAG dep_edges, so a test body refing a sibling recipe races under
# --jobs > 1.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    exit 1
fi

pass=0
fail=0
scenario=0

clean_state() {
    "$COOK" clean >/dev/null 2>&1 || true
    find . -type d -name ".cook" -exec rm -rf {} + 2>/dev/null || true
    find . -type d -name "build" -exec rm -rf {} + 2>/dev/null || true
}

echo "=== cross_cookfile_test bug-hunt ==="
echo

echo "--- Scenario 1: cook --test --jobs 1 (deterministic order) ---"
clean_state
scenario=$((scenario + 1))
out=$("$COOK" --test --jobs 1 2>&1)
rc=$?
printf "  [%d] %-60s " "$scenario" "cook --test --jobs 1 exits 0"
if [ "$rc" -eq 0 ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (exit $rc)"
    echo "$out" | sed 's/^/       /' | tail -15
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 2: cook --test --jobs 4 (race-prone per CS-0027) ---"
race_failures=0
for i in 1 2 3 4 5; do
    clean_state
    if ! "$COOK" --test --jobs 4 >/dev/null 2>&1; then
        race_failures=$((race_failures + 1))
    fi
done
scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "cook --test --jobs 4 (5 trials)"
if [ "$race_failures" -eq 0 ]; then
    echo "PASS (0/5 races)"
    pass=$((pass + 1))
else
    echo "RACED ($race_failures/5 trials failed)"
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 3: cook verify_gen explicit (no --test flag) ---"
clean_state
scenario=$((scenario + 1))
out=$("$COOK" verify_gen 2>&1)
rc=$?
printf "  [%d] %-60s " "$scenario" "cook verify_gen exits 0"
if [ "$rc" -eq 0 ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (exit $rc)"
    echo "$out" | sed 's/^/       /' | tail -10
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
