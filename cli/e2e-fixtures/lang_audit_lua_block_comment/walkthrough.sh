#!/bin/bash
# CS-0035 audit walkthrough — multi-line Lua block comments.
#
# Asserts that `cook emit-lua` parses a Cookfile whose `>{ ... }` Lua
# block contains a multi-line `--[[ ... ]]` and `--[==[ ... ]==]` block
# comment, each with `}` bytes interior to the comment. Pre-CS-0035 the
# parser rejected this with "unclosed Lua block".

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

echo "=== lang_audit_lua_block_comment (CS-0035) ==="
echo

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

printf "  [2] %-60s " "block-comment body present in emitted Lua"
if echo "$out" | grep -q "multi-line block comment with a }"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (block-comment body missing)"
    fail=$((fail + 1))
fi

printf "  [3] %-60s " "leveled block-comment body present"
if echo "$out" | grep -q "leveled block comment; ]] does NOT close it"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (leveled block-comment body missing)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
