#!/bin/bash
# walkthrough.sh — pin the cook CLI exit-code taxonomy.
#
# Maps to CookError::exit_code in cli/crates/cook-cli/src/error.rs:
#   CommandFailed(_)  -> 1
#   TestFailure(_)    -> 1
#   ParseError(_)     -> 2
#   RecipeNotFound(_) -> 3
#   Other(_)          -> 1
#
# This is the runtime pin for the spec's exit-code table.

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

check_exit() {
    local label="$1"
    local expected="$2"
    local actual="$3"
    scenario=$((scenario + 1))
    printf "  [%d] %-60s " "$scenario" "$label"
    if [ "$actual" -eq "$expected" ]; then
        echo "PASS (exit $actual)"
        pass=$((pass + 1))
    else
        echo "FAIL (expected $expected, got $actual)"
        fail=$((fail + 1))
    fi
}

echo "=== cli_audit_exit_codes ==="
echo

# Exit 0 — successful recipe
clean_state
"$COOK" build >/dev/null 2>&1
check_exit "successful recipe build" 0 "$?"

# Exit 1 — failing shell command (CommandFailed)
clean_state
"$COOK" boom >/dev/null 2>&1
check_exit "failing shell command (CommandFailed)" 1 "$?"

# Exit 3 — unknown recipe (RecipeNotFound)
clean_state
"$COOK" no_such_recipe >/dev/null 2>&1
check_exit "unknown recipe (RecipeNotFound)" 3 "$?"

# Exit 2 — parse error: malformed Cookfile
clean_state
tmpdir=$(mktemp -d)
# `recipe` keyword with no name is a parse error.
printf "recipe\n" > "$tmpdir/Cookfile"
"$COOK" -f "$tmpdir/Cookfile" build >/dev/null 2>&1
check_exit "parse error (ParseError)" 2 "$?"
rm -rf "$tmpdir"

clean_state

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
