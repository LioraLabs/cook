#!/bin/bash
# walkthrough.sh — pin v1.0 test-result caching contract.
#
# Replaces the Phase 8 stub. Asserts:
#   1. First run (clean state): no cache hits in output
#   2. Second run: all passing tests show "cached" in summary
#   3. --rerun busts cache: no "cached" in output
#   4. After --rerun, next plain run re-stores cache entries
#   5. Source edit invalidates cached results WITHOUT --rerun (COOK-84)

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

clean() { rm -rf .cook build; }

# ---------------------------------------------------------------------------
# 1. First run: no cache hits (clean state, everything runs fresh)
# ---------------------------------------------------------------------------
clean
out1=$("$COOK" test 2>&1 || true)
if echo "$out1" | grep -qF "cached"; then
    echo "FAIL: first run (clean) should have no cache hits"
    echo "----- output -----"
    echo "$out1"
    echo "------------------"
    fail=$((fail + 1))
else
    echo "PASS: first run has no cache hits"
    pass=$((pass + 1))
fi

# ---------------------------------------------------------------------------
# 2. Second run: every passing test is replayed from cache
# ---------------------------------------------------------------------------
out2=$("$COOK" test 2>&1 || true)
if echo "$out2" | grep -qF "cached"; then
    echo "PASS: second run shows cache hits"
    pass=$((pass + 1))
else
    echo "FAIL: second run has no cache hits — caching not working"
    echo "----- output -----"
    echo "$out2"
    echo "------------------"
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# 3. --rerun busts everything: no "cached" in output
# ---------------------------------------------------------------------------
out3=$("$COOK" test --rerun 2>&1 || true)
if echo "$out3" | grep -qF "cached"; then
    echo "FAIL: --rerun should produce no cache hits"
    echo "----- output -----"
    echo "$out3"
    echo "------------------"
    fail=$((fail + 1))
else
    echo "PASS: --rerun busts cache (no cached hits)"
    pass=$((pass + 1))
fi

# ---------------------------------------------------------------------------
# 4. Post-rerun: next plain run re-stores and hits cache
# ---------------------------------------------------------------------------
out4=$("$COOK" test 2>&1 || true)
if echo "$out4" | grep -qF "cached"; then
    echo "PASS: post-rerun cache rebuilt (hits on next plain run)"
    pass=$((pass + 1))
else
    echo "FAIL: post-rerun plain run did not rebuild cache"
    echo "----- output -----"
    echo "$out4"
    echo "------------------"
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# 5. Source-edit invalidation (COOK-84): breaking a source file must fail
#    the next plain run — the fingerprint alone notices, no --rerun needed.
# ---------------------------------------------------------------------------
src_file=$(ls src/*.txt | head -1)
orig=$(cat "$src_file")
echo "broken" > "$src_file"
out5=$("$COOK" test 2>&1 || true)
echo "$orig" > "$src_file"
if echo "$out5" | grep -qE "failed"; then
    echo "PASS: source edit invalidated cached test (run failed without --rerun)"
    pass=$((pass + 1))
else
    echo "FAIL: source edit did not invalidate cache — stale pass replayed"
    echo "----- output -----"
    echo "$out5"
    echo "------------------"
    fail=$((fail + 1))
fi

# Re-warm the cache so the example is left in a hot state.
"$COOK" test > /dev/null 2>&1 || true

echo
echo "Passed: $pass   Failed: $fail"
exit $((fail > 0 ? 1 : 0))
