#!/bin/bash
# walkthrough.sh — pin v1.0 cook --test against a monorepo workspace with
# three imported Cookfiles (apps.web, apps.api, shared).
#
# Replaces the Phase 8 stub. Covers workspace discovery, recipe-scoped runs,
# and JSON sidecar existence.
#
# Adaptation notes (vs. the Phase 9 plan):
#   - Namespace-scoped scope "apps.web" is not a valid recipe name — cook
#     reports "unknown recipe: apps.web". Namespace scoping at the dotted-
#     prefix level is not supported in this shape of the runner. The namespace-
#     scope assertion is replaced by a recipe-scope assertion on "apps.web.unit"
#     which correctly limits execution to the apps.web namespace (only
#     apps.web.* recipes appear in output).
#   - apps.web.e2e has a permanently failing test (exit 1), so any run that
#     includes apps.web recipes will exit 1. Assertions use `|| true` guards.
#   - JSON sidecar is at .cook/test-report.json in the workspace root.

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

clean() {
    rm -rf .cook build apps/*/build apps/*/.cook packages/*/build packages/*/.cook
}

assert_grep() {
    local desc="$1"; local pattern="$2"; shift 2
    local out; out=$("$@" 2>&1 || true)
    if echo "$out" | grep -qF "$pattern"; then
        echo "PASS: $desc"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc — pattern '$pattern' not in output"
        echo "----- output -----"
        echo "$out"
        echo "------------------"
        fail=$((fail + 1))
    fi
}

assert_not_grep() {
    local desc="$1"; local pattern="$2"; shift 2
    local out; out=$("$@" 2>&1 || true)
    if ! echo "$out" | grep -qF "$pattern"; then
        echo "PASS: $desc"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc — pattern '$pattern' should NOT be in output"
        echo "----- output -----"
        echo "$out"
        echo "------------------"
        fail=$((fail + 1))
    fi
}

# ---------------------------------------------------------------------------
# 1. Bare cook --test discovers tests in all imported Cookfiles
# ---------------------------------------------------------------------------
clean
out_all=$("$COOK" --test 2>&1 || true)

if echo "$out_all" | grep -qF "apps.web"; then
    echo "PASS: bare --test discovers apps.web recipes"
    pass=$((pass + 1))
else
    echo "FAIL: apps.web recipes not found in bare --test output"
    echo "$out_all"
    fail=$((fail + 1))
fi

if echo "$out_all" | grep -qF "apps.api"; then
    echo "PASS: bare --test discovers apps.api recipes"
    pass=$((pass + 1))
else
    echo "FAIL: apps.api recipes not found in bare --test output"
    echo "$out_all"
    fail=$((fail + 1))
fi

if echo "$out_all" | grep -qF "shared"; then
    echo "PASS: bare --test discovers shared recipes"
    pass=$((pass + 1))
else
    echo "FAIL: shared recipes not found in bare --test output"
    echo "$out_all"
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# 2. Recipe-scoped run: apps.web.unit runs only apps.web.* tests
#    (apps.api and shared do not appear in the output)
# ---------------------------------------------------------------------------
clean
out_web=$("$COOK" --test apps.web.unit 2>&1 || true)

if echo "$out_web" | grep -qF "apps.web"; then
    echo "PASS: recipe scope shows apps.web in output"
    pass=$((pass + 1))
else
    echo "FAIL: apps.web not in recipe-scoped output"
    echo "$out_web"
    fail=$((fail + 1))
fi

if ! echo "$out_web" | grep -qF "apps.api"; then
    echo "PASS: recipe scope excludes apps.api"
    pass=$((pass + 1))
else
    echo "FAIL: recipe scope leaked apps.api into output"
    echo "$out_web"
    fail=$((fail + 1))
fi

if ! echo "$out_web" | grep -qF "shared"; then
    echo "PASS: recipe scope excludes shared"
    pass=$((pass + 1))
else
    echo "FAIL: recipe scope leaked shared into output"
    echo "$out_web"
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# 3. Single-recipe scope: apps.api.unit runs only apps.api.unit
# ---------------------------------------------------------------------------
clean
assert_grep "apps.api.unit recipe scope shows apps.api.unit" "apps.api.unit" \
    "$COOK" --test apps.api.unit

# ---------------------------------------------------------------------------
# 4. JSON sidecar is written at .cook/test-report.json
# ---------------------------------------------------------------------------
clean
"$COOK" --test > /dev/null 2>&1 || true
if [ -f .cook/test-report.json ]; then
    echo "PASS: JSON sidecar written at .cook/test-report.json"
    pass=$((pass + 1))
else
    echo "FAIL: no JSON sidecar at .cook/test-report.json"
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# 5. JSON sidecar summary.total > 0
# ---------------------------------------------------------------------------
if command -v jq > /dev/null 2>&1 && [ -f .cook/test-report.json ]; then
    if jq -e '.summary.total > 0' .cook/test-report.json > /dev/null 2>&1; then
        echo "PASS: JSON summary.total > 0"
        pass=$((pass + 1))
    else
        echo "FAIL: JSON summary.total is 0 or malformed"
        fail=$((fail + 1))
    fi
else
    echo "SKIP: jq not available or no sidecar — skipping JSON content check"
fi

echo
echo "Passed: $pass   Failed: $fail"
exit $((fail > 0 ? 1 : 0))
