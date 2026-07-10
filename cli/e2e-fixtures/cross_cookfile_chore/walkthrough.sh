#!/bin/bash
# Chore-side companion demonstrating body-ref dep propagation. A chore
# whose body refs an imported recipe via $<lib.gen> instead of a `test`
# step. Chores lower to cook.add_unit (same as a `cook "out" { }` step),
# which propagates step_group_dep_refs into dep_edges, so the wave grouper
# sequences lib.gen before consume_gen even at high job counts — unlike
# `test`, which lowers to cook.add_test and drops those refs.

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

echo "=== cross_cookfile_chore positive-coverage ==="
echo

echo "--- Scenario 1: cook consume_gen --jobs 1 ---"
clean_state
scenario=$((scenario + 1))
printf "  [%d] %-55s " "$scenario" "cook consume_gen --jobs 1"
if "$COOK" consume_gen --jobs 1 >/dev/null 2>&1; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 2: cook consume_gen --jobs 4, 10 fresh-state trials ---"
race_failures=0
for i in 1 2 3 4 5 6 7 8 9 10; do
    clean_state
    if ! "$COOK" consume_gen --jobs 4 >/dev/null 2>&1; then
        race_failures=$((race_failures + 1))
    fi
done
scenario=$((scenario + 1))
printf "  [%d] %-55s " "$scenario" "cook consume_gen --jobs 4 (10 trials)"
if [ "$race_failures" -eq 0 ]; then
    echo "PASS (0/10 races)"
    pass=$((pass + 1))
else
    echo "RACED ($race_failures/10 trials failed)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
