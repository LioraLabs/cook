#!/bin/bash
# CS-0035 audit walkthrough — multi-line Lua long strings.
#
# Asserts that `cook emit-lua` parses a Cookfile whose `>{ ... }` Lua
# block contains a multi-line `[[ ... ]]` and `[==[ ... ]==]` long string,
# each with `}` bytes interior to the string. Pre-CS-0035 the parser
# rejected this with "unclosed Lua block".

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it with: (cd ../../cli && cargo build -p cook-cli)"
    exit 1
fi

pass=0
fail=0

echo "=== lang_audit_lua_multiline_string (CS-0035) ==="
echo

# Scenario 1: parse succeeds.
out=$("$COOK" emit-lua 2>&1)
rc=$?
printf "  [1] %-60s " "cook emit-lua exits 0 (parser accepts)"
if [ "$rc" -eq 0 ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (exit $rc)"
    echo "$out" | sed 's/^/       /' | tail -10
    fail=$((fail + 1))
fi

# Scenario 2: emitted Lua contains the long-string content verbatim.
printf "  [2] %-60s " "long-string body present in emitted Lua"
if echo "$out" | grep -q "this Lua long string contains a }"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (long-string body missing or corrupted)"
    fail=$((fail + 1))
fi

# Scenario 3: emitted Lua contains the leveled long-string content.
printf "  [3] %-60s " "[==[ ... ]==] level-2 body present"
if echo "$out" | grep -q "leveled long string with } and ]] inside"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (level-2 long string body missing or corrupted)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
