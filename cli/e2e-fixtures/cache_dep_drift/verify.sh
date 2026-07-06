#!/bin/bash
# verify.sh — focused regression for 2026-05-02 addendum §4.3.
#
# Asserts that a content change in a dep recipe's output invalidates the
# consumer's cache. Pre-fix, the consumer would silently hit cache because
# the dep output's path was never recorded in cache_meta.input_paths.

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
    printf "  [%d] %-55s " "$scenario" "$name"
    local out
    out="$("$@" 2>&1 | grep -oE 'cook build done.*\)' | head -1)"
    if [[ "$out" == *"$expected"* ]]; then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL"
        echo "      expected substring: $expected"
        echo "      got:                $out"
        fail=$((fail + 1))
    fi
}

echo "=== cache dep-output drift (addendum §4.3 regression) ==="
echo

# Reset to a known state. We snapshot src/lib.c so the mutation below
# can be undone whether or not the file is tracked in git yet.
"$COOK" clean >/dev/null 2>&1
trap 'cp -f src/lib.c.bak src/lib.c 2>/dev/null; rm -f src/lib.c.bak' EXIT
cp -f src/lib.c src/lib.c.bak

run_assert "Fresh build: both execute"        "0 cached recipes, 2 done"  "$COOK" app
run_assert "No-op rebuild: both hit"          "2 cached recipes, 2 done"  "$COOK" app

# Force lib.o content drift WITHOUT touching main.c. Pre-fix, app's
# StepEntry.inputs is just [src/main.c] which is unchanged → app hits
# cache → BUG (app linked against the old lib.o hash). Post-fix,
# build/lib.o is in app's cache_meta.input_paths and its content has
# drifted → InputChanged → app rebuilds.
printf '\nint lib_drift_sentinel(void) { return 42; }\n' >> src/lib.c

scenario=$((scenario + 1))
printf "  [%d] %-55s " "$scenario" "lib.c drift → BOTH rebuild (consumer not silent)"
out="$("$COOK" app 2>&1 | grep -oE 'cook build done.*\)' | head -1)"
# Tolerant assertion: the critical signal is "0 cached" — neither lib_obj nor
# app is allowed to cache-hit. A pre-fix run prints "1 cached recipes, 2 done"
# (app silently hit because lib.o wasn't in its inputs).
if [[ "$out" == *"0 cached recipes, 2 done"* ]]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "      expected:  0 cached recipes, 2 done"
    echo "      got:       $out"
    echo "      (pre-fix this would say '1 cached recipes, 2 done' — app silently hit)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
