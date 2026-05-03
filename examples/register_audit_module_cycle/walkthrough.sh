#!/bin/bash
# Audit fixture for CS-0035: cook.load_module memoization + cycle detection.
#
# Pre-CS-0035 the register-phase loader (cli/crates/cook-register/src/
# module_loader.rs) had no in-flight set and no memoization table. A two-
# module cycle (a -> b -> a) re-entered the loader indefinitely until the
# native stack overflowed. CS-0035 (a) caches successful loads so repeated
# imports return the same Lua value, and (b) tracks an in-flight set so
# re-entrant calls raise `module cycle detected: a -> b -> a` with the
# offending path rendered.
#
# Each scenario asserts on the loader's behaviour. The script exits 0 only
# when every scenario passes.

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

run_assert_diag() {
    # Args: name, expected_substring, cookfile, recipe
    local name="$1"
    local expected="$2"
    local cookfile="$3"
    local recipe="$4"
    scenario=$((scenario + 1))
    printf "  [%d] %-60s " "$scenario" "$name"

    local out rc
    out="$(timeout 10s "$COOK" -f "$cookfile" "$recipe" 2>&1)"
    rc=$?

    # Cycle and self-cycle MUST exit non-zero (rejected, not stack overflowed)
    # AND the diagnostic MUST contain the expected substring.
    if [ "$rc" -eq 0 ]; then
        echo "FAIL (cook exited 0; expected non-zero)"
        echo "       output: $(echo "$out" | head -5 | sed 's/^/         /')"
        fail=$((fail + 1))
        return
    fi
    if [ "$rc" -eq 124 ]; then
        echo "FAIL (timeout — likely infinite recursion / stack overflow)"
        fail=$((fail + 1))
        return
    fi
    if echo "$out" | grep -qF "$expected"; then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL (diagnostic missing expected substring)"
        echo "       expected substring: $expected"
        echo "       got:"
        echo "$out" | sed 's/^/         /'
        fail=$((fail + 1))
    fi
}

run_assert_success() {
    # Args: name, expected_stdout_substring, cookfile, recipe
    local name="$1"
    local expected="$2"
    local cookfile="$3"
    local recipe="$4"
    scenario=$((scenario + 1))
    printf "  [%d] %-60s " "$scenario" "$name"

    local out rc
    out="$(timeout 10s "$COOK" -f "$cookfile" "$recipe" 2>&1)"
    rc=$?

    if [ "$rc" -ne 0 ]; then
        echo "FAIL (cook exited $rc; expected 0)"
        echo "       output:"
        echo "$out" | sed 's/^/         /'
        fail=$((fail + 1))
        return
    fi
    if echo "$out" | grep -qF "$expected"; then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL (output missing expected substring)"
        echo "       expected substring: $expected"
        echo "       got:"
        echo "$out" | sed 's/^/         /'
        fail=$((fail + 1))
    fi
}

clean_state() {
    rm -rf .cook 2>/dev/null || true
}

echo "=== register_audit_module_cycle (CS-0035) ==="
echo

echo "--- Scenario 1: 2-module cycle a -> b -> a (was infinite recursion) ---"
clean_state
run_assert_diag "cook.load_module(\"a\") with a->b->a cycle" \
    "module cycle detected: a -> b -> a" \
    "Cookfile.cycle" "audit_cycle"

echo
echo "--- Scenario 2: self-cycle solo -> solo ---"
clean_state
run_assert_diag "cook.load_module(\"solo\") that self-loads" \
    "module cycle detected: solo -> solo" \
    "Cookfile.self" "audit_self"

echo
echo "--- Scenario 3: memoization (top-level + init() each run once) ---"
clean_state
run_assert_success "cook.load_module(\"once\") x3 must hit cache" \
    "memoization OK: top-level=1 init=1" \
    "Cookfile.memo" "audit_memo"

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
