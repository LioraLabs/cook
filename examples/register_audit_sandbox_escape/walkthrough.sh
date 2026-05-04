#!/bin/bash
# Audit fixture for CS-0045: project-root sandbox for cook-step Lua bodies.
#
# Pre-CS-0045 the Cook Lua API table `fs.*` resolved every relative path
# against the recipe's working directory but applied no upper bound: a
# cook-step body could read `/etc/passwd`, write `/tmp/leaked`, traverse
# out via `../../etc`, or shell out via `os.execute("rm -rf /")` and the
# implementation would happily comply. CS-0045 closes that gap by
# confining `fs.*` and the Lua-side shell escape hatches (`os.execute`,
# `io.popen`) to the project root in cook/test/chore step contexts. Plate
# steps remain unconstrained because shipping outside the project root is
# their explicit purpose.
#
# Each scenario asserts on the diagnostic. The script exits 0 only when
# every scenario passes.

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

echo "=== register_audit_sandbox_escape (CS-0045) ==="
echo

echo "--- Scenario 1: cook step body fs.read of absolute outside-root path ---"
clean_state
run_assert_diag "fs.read(\"/etc/passwd\") MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.absolute" "sandbox_probe"

echo
echo "--- Scenario 2: register-phase config block fs.read leak ---"
clean_state
run_assert_diag "config { fs.read(\"/etc/passwd\") } MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.config" "register_phase_probe"

echo
echo "--- Scenario 3: cook step body os.execute escape hatch ---"
clean_state
run_assert_diag "os.execute(\"...\") in cook step MUST raise CS-0045" \
    "CS-0045" \
    "Cookfile.os_execute" "os_execute_probe"

echo
echo "--- Scenario 4: plate step body is NOT sandboxed (deliberate) ---"
clean_state
run_assert_success "plate body fs.read /etc/hostname + os.execute MUST succeed" \
    "plate-ok" \
    "Cookfile.plate_ok" "ship"

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
