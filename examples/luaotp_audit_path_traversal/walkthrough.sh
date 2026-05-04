#!/bin/bash
# Audit fixture for CS-0045: path traversal and shell escape rejection
# in execute-phase Lua bodies.
#
# Companion to register_audit_sandbox_escape: that fixture exercises
# the register-phase VM (config blocks, top-level Lua) and the
# execute-phase cook-step body. This fixture concentrates on
# execute-phase corner cases — relative `..` traversal, absolute
# fs.write, fs.glob with an outside pattern, io.popen — and pins one
# explicit no-regression: a path inside the project root MUST still
# succeed.
#
# Each scenario asserts on the diagnostic. The script exits 0 only
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
    local name="$1"
    local expected="$2"
    local cookfile="$3"
    local recipe="$4"
    scenario=$((scenario + 1))
    printf "  [%d] %-60s " "$scenario" "$name"

    local out rc
    out="$(timeout 10s "$COOK" -f "$cookfile" "$recipe" 2>&1)"
    rc=$?

    if [ "$rc" -eq 0 ]; then
        echo "FAIL (cook exited 0; expected non-zero)"
        echo "       output: $(echo "$out" | head -5 | sed 's/^/         /')"
        fail=$((fail + 1))
        return
    fi
    if [ "$rc" -eq 124 ]; then
        echo "FAIL (timeout)"
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
    # Scenario 5 writes an in-project file to prove the no-regression
    # case; clean it up so a re-run starts from a fresh state.
    rm -f inside.txt 2>/dev/null || true
    # Pre-clean the file the leak scenario would create on a buggy
    # implementation, so a stale file from a previous run can't paper
    # over a regression.
    rm -f /tmp/cs_0045_should_not_exist.txt 2>/dev/null || true
}

echo "=== luaotp_audit_path_traversal (CS-0045) ==="
echo

echo "--- Scenario 1: relative '../' traversal MUST be rejected ---"
clean_state
run_assert_diag "fs.read(\"../../../etc/passwd\") MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.dotdot" "traversal_probe"

echo
echo "--- Scenario 2: fs.write to absolute outside path MUST be rejected ---"
clean_state
run_assert_diag "fs.write(\"/tmp/...\", ...) MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.write_outside" "write_outside_probe"

# Sanity check: the leak file MUST NOT exist on disk after the call.
if [ -f /tmp/cs_0045_should_not_exist.txt ]; then
    echo "  [extra] FAIL: /tmp/cs_0045_should_not_exist.txt was written"
    fail=$((fail + 1))
else
    echo "  [extra] PASS: /tmp/cs_0045_should_not_exist.txt was NOT written"
    pass=$((pass + 1))
fi

echo
echo "--- Scenario 3: fs.glob with absolute outside pattern MUST be rejected ---"
clean_state
run_assert_diag "fs.glob(\"/etc/*\") MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.glob_outside" "glob_outside_probe"

echo
echo "--- Scenario 4: io.popen escape hatch MUST be rejected ---"
clean_state
run_assert_diag "io.popen(\"id\") in cook step MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.io_popen" "io_popen_probe"

echo
echo "--- Scenario 5: confined path inside project root MUST succeed ---"
clean_state
run_assert_success "fs.write+read of project-relative path MUST succeed" \
    "inside-ok:ok" \
    "Cookfile.inside_ok" "inside_probe"

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
