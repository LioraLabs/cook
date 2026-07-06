#!/bin/bash
# Bug-hunt fixture: same-name recipes across Cookfiles + explicit `:` cross-
# Cookfile deps + chore depending on imported recipes.
#
# Each scenario asserts on cook's tail line. Don't fix issues — just record
# what each scenario produces.

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

run_assert() {
    local name="$1"
    local expected="$2"
    shift 2
    scenario=$((scenario + 1))
    printf "  [%d] %-60s " "$scenario" "$name"
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

run_expect_failure() {
    local name="$1"
    shift
    scenario=$((scenario + 1))
    printf "  [%d] %-60s " "$scenario" "$name"
    local rc
    "$@" >/dev/null 2>&1
    rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "PASS (non-zero exit, as expected)"
        pass=$((pass + 1))
    else
        echo "FAIL (exited 0 — should have errored)"
        fail=$((fail + 1))
    fi
}

clean_state() {
    "$COOK" clean >/dev/null 2>&1 || true
    find . -type d -name ".cook" -exec rm -rf {} + 2>/dev/null || true
    find . -type d -name "build" -exec rm -rf {} + 2>/dev/null || true
}

echo "=== cross_cookfile_naming bug-hunt ==="
echo

echo "--- Scenario 1: assemble fresh build (3 nodes via explicit ':' deps) ---"
clean_state
run_assert "cook assemble (fresh)" "0 cached recipes, 3 done" "$COOK" assemble

echo
echo "--- Scenario 2: assemble rerun (full cache) ---"
run_assert "cook assemble (cached)" "3 cached recipes, 3 done" "$COOK" assemble

echo
echo "--- Scenario 3: cli.build drift only invalidates cli + assemble ---"
echo "cli updated $(date)" >> apps/cli/src.txt
run_assert "cook assemble after cli drift" "1 cached recipes, 3 done" "$COOK" assemble
echo "cli source content" > apps/cli/src.txt

echo
echo "--- Scenario 4: chore deploy fires, both build recipes prereq ---"
clean_state
out=$("$COOK" deploy 2>&1)
scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "cook deploy from root"
if echo "$out" | grep -q 'cli artifact:' && echo "$out" | grep -q 'server artifact:'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       deploy output:"
    echo "$out" | sed 's/^/       /'
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 5: bare 'build' from root should be ambiguous/unknown ---"
run_expect_failure "cook build from root (no recipe named 'build' at root)" "$COOK" build

echo
echo "--- Scenario 6: 'build' from apps/cli/ subdir, .cookroot walk-up ---"
clean_state
sub_out=$(cd apps/cli && "$COOK" build 2>&1)
scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "cook build from apps/cli/ subdir"
if echo "$sub_out" | grep -q 'cook build done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got:"
    echo "$sub_out" | sed 's/^/       /' | tail -10
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
