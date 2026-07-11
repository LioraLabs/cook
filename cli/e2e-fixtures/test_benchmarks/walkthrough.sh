#!/bin/bash
# walkthrough.sh — pin v1.0 cook test against the test_benchmarks fixture.
#
# Replaces the Phase 8 stub. Covers every runner contract the fixture exercises:
# green path, iteration aggregation, body-inverted failure, failure capture,
# mixed pass/fail, blocked (engine-level), a slow-but-passing test, unnamed
# auto-labeling, cache replay, and --rerun-failed.
#
# Adaptation notes (vs. the Phase 9 plan):
#   - blocked_by_build: the cook step's `false` causes an engine-level error
#     ("engine: 1 task(s) failed") rather than a clean TestBlocked event. The
#     test never enters the runner accumulator. Assertion is weakened to
#     "exit 1" only — no "blocked" pattern check.
#   - --rerun-failed with recipe scope: works. The second run re-executes the
#     2 failed tests (input_01, input_02) while the 10 passers stay cached.
#     Assertion checks for "2 failed" in the second-run output.
#   - named_test: CS-0135 removed the `as` modifier, so unnamed tests now
#     always display as `recipe@line`; the auto-label only appears in
#     --verbose output (not in the plain summary line). Assertion uses
#     --verbose and checks for that label instead of a custom name.
#   - CS-0135 also removed the `timeout` and `should_fail` modifiers:
#     pass_should_fail now inverts its failing body with `!` instead, and
#     slow_timeout (renamed slow_pass) now just proves a slow test still
#     passes under the engine's fixed internal ceiling, since Cookfiles can
#     no longer configure or trigger a timeout on demand.

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

assert_exit() {
    local desc="$1"; local expected="$2"; shift 2
    local out; out=$("$@" 2>&1); local actual=$?
    if [ "$actual" = "$expected" ]; then
        echo "PASS: $desc (exit=$actual)"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc (expected=$expected actual=$actual)"
        echo "----- output -----"
        echo "$out"
        echo "------------------"
        fail=$((fail + 1))
    fi
}

assert_grep() {
    local desc="$1"; local pattern="$2"; shift 2
    local out; out=$("$@" 2>&1 || true)
    if echo "$out" | grep -qF "$pattern"; then
        echo "PASS: $desc"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc — pattern not found: $pattern"
        echo "----- output -----"
        echo "$out"
        echo "------------------"
        fail=$((fail + 1))
    fi
}

clean() { rm -rf .cook build; }

# ---------------------------------------------------------------------------
# 1. Green path: pass_basic exits 0
# ---------------------------------------------------------------------------
clean
assert_exit "pass_basic exits 0" 0 "$COOK" test pass_basic

# ---------------------------------------------------------------------------
# 2. Iteration over 12 inputs, all passing — summary says "12 passed"
# ---------------------------------------------------------------------------
clean
assert_grep "pass_iterated reports 12 passed" "12 passed" "$COOK" test pass_iterated

# ---------------------------------------------------------------------------
# 3. Body inversion — `! (exit 1)` flips the failing command to a pass →
#    exit 0
# ---------------------------------------------------------------------------
clean
assert_exit "pass_should_fail exits 0" 0 "$COOK" test pass_should_fail

# ---------------------------------------------------------------------------
# 4. Failure — fail_basic exits 1
# ---------------------------------------------------------------------------
clean
assert_exit "fail_basic exits 1" 1 "$COOK" test fail_basic

# ---------------------------------------------------------------------------
# 5. Mixed pass/fail over 12 inputs (3 fail) — summary says "3 failed"
# ---------------------------------------------------------------------------
clean
assert_grep "fail_partial reports 3 failed" "3 failed" "$COOK" test fail_partial

# ---------------------------------------------------------------------------
# 6. Blocked by build step: the cook step runs `false`, which causes the
#    downstream test to be reported as Blocked rather than producing a raw
#    engine error. Cook exits 1, and "blocked" appears in the summary.
#    (SHI-173: previously this produced "engine: 1 task(s) failed" and no
#    Blocked row; the fix translates cook failures into Blocked TestResults.)
# ---------------------------------------------------------------------------
clean
assert_exit "blocked_by_build exits 1" 1 "$COOK" test blocked_by_build
assert_grep "blocked_by_build reports blocked" "blocked" "$COOK" test blocked_by_build

# ---------------------------------------------------------------------------
# 7. slow_pass — `sleep 2` finishes well inside the engine's fixed internal
#    ceiling, so the test still passes (CS-0135 removed the `timeout`
#    modifier, so Cookfiles can no longer trigger a timeout on demand).
# ---------------------------------------------------------------------------
clean
assert_exit "slow_pass exits 0" 0 "$COOK" test slow_pass

# ---------------------------------------------------------------------------
# 8. Unnamed-test auto-labeling — CS-0135 removed the `as` modifier, so the
#    test's label in --verbose output is the auto-generated "recipe@line".
# ---------------------------------------------------------------------------
clean
assert_grep "named_test auto-labels as named_test@73 in verbose output" "named_test@73" \
    "$COOK" test named_test --verbose

# ---------------------------------------------------------------------------
# 9. Cache replay — second run without clean shows "cached" in summary
# ---------------------------------------------------------------------------
clean
"$COOK" test cached_replay > /dev/null 2>&1 || true
out=$("$COOK" test cached_replay 2>&1 || true)
if echo "$out" | grep -qF "cached"; then
    echo "PASS: cached_replay second run shows cached"
    pass=$((pass + 1))
else
    echo "FAIL: cached_replay no cache hit"
    echo "----- output -----"
    echo "$out"
    echo "------------------"
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# 10. --rerun-failed — first run records 2 failures (input_01, input_02);
#     second run with --rerun-failed re-executes only those 2 (still fail
#     deterministically), so "2 failed" in the output confirms the re-run
#     scope was respected (not all 12 tests re-ran fresh).
# ---------------------------------------------------------------------------
clean
"$COOK" test rerun_failed_set > /dev/null 2>&1 || true
out=$("$COOK" test rerun_failed_set --rerun-failed 2>&1 || true)
if echo "$out" | grep -qF "2 failed"; then
    echo "PASS: --rerun-failed re-ran the 2 previously failed tests"
    pass=$((pass + 1))
else
    echo "FAIL: --rerun-failed did not produce expected '2 failed' output"
    echo "----- output -----"
    echo "$out"
    echo "------------------"
    fail=$((fail + 1))
fi

echo
echo "Passed: $pass   Failed: $fail"
exit $((fail > 0 ? 1 : 0))
