#!/bin/bash
# walkthrough.sh — pin that `cook --test` exits non-zero on a failing test.
#
# Pre-fix (pipeline.rs cmd_test): `let _result = run_with_progress(...)` then
# `Ok(())`. Failing tests silently exited 0. Post-fix: the result is `?`-d
# and any test failure maps to CookError::CommandFailed (exit 1) via
# engine_error_to_cook_error's COOK_CMD_FAILED branch.

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
    rm -rf .cook 2>/dev/null || true
}

echo "=== cli_audit_test_exit_code ==="
echo

# Scenario 1 — failing test must surface a non-zero exit
echo "--- Scenario 1: cook --test --jobs 1 with a failing test recipe ---"
clean_state
out=$("$COOK" --test --jobs 1 2>&1)
rc=$?
scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "cook --test exits non-zero"
if [ "$rc" -ne 0 ]; then
    echo "PASS (exit $rc)"
    pass=$((pass + 1))
else
    echo "FAIL (exit 0 -- test failure was silently swallowed)"
    echo "$out" | sed 's/^/       /' | tail -10
    fail=$((fail + 1))
fi

# Scenario 2 — only the passing recipe should leave exit 0
echo
echo "--- Scenario 2: cook --test against a Cookfile with only passing tests ---"
clean_state
# Use a temporary Cookfile that only has passing tests.
tmp_cookfile=$(mktemp -d)/Cookfile
cat > "$tmp_cookfile" <<'EOF'
recipe passing_only
    test {
        true
    }
EOF
out=$("$COOK" -f "$tmp_cookfile" --test --jobs 1 2>&1)
rc=$?
scenario=$((scenario + 1))
printf "  [%d] %-60s " "$scenario" "passing-only suite exits 0"
if [ "$rc" -eq 0 ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (exit $rc)"
    echo "$out" | sed 's/^/       /' | tail -10
    fail=$((fail + 1))
fi

clean_state

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
