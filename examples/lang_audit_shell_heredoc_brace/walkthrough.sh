#!/bin/bash
# CS-0035 audit walkthrough — shell heredocs with `}` braces in body.
#
# Asserts that `cook --emit-lua` parses a Cookfile whose `using { ... }`
# shell block contains POSIX heredocs in three forms (bare, single-quoted
# delimiter, dash form) and that `}` bytes on heredoc-body lines do not
# prematurely close the surrounding shell block.

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

echo "=== lang_audit_shell_heredoc_brace (CS-0035) ==="
echo

out=$("$COOK" --emit-lua 2>&1)
rc=$?
printf "  [1] %-60s " "cook --emit-lua exits 0 (parser accepts)"
if [ "$rc" -eq 0 ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (exit $rc)"
    echo "$out" | sed 's/^/       /' | tail -10
    fail=$((fail + 1))
fi

printf "  [2] %-60s " "bare-heredoc body present in emitted Lua"
if echo "$out" | grep -q "bare heredoc with a } brace"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (bare-heredoc body missing)"
    fail=$((fail + 1))
fi

printf "  [3] %-60s " "single-quoted-delim body present"
if echo "$out" | grep -q "single-quoted delimiter; } is literal"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (quoted-delim heredoc body missing)"
    fail=$((fail + 1))
fi

printf "  [4] %-60s " "dash-form heredoc body present"
if echo "$out" | grep -q "tab-stripped form; } also literal"; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL (dash-form heredoc body missing)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
