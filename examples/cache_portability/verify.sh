#!/bin/bash
# Bug-hunt fixture: §7.7 cache portability — content-addressed cache keys
# should not include absolute paths.
#
# Procedure:
#   1. Build at /tmp/cook-portability-A/
#   2. cp -r the entire workspace (including .cook/) to /tmp/cook-portability-B/
#   3. Run cook again at /tmp/cook-portability-B/
#   4. Expect: full cache hit (1 cached recipes, 1 done)
#
# If the cache key includes any absolute path, the move invalidates and
# this fails.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    exit 1
fi

A=/tmp/cook-portability-A
B=/tmp/cook-portability-B
rm -rf "$A" "$B"
mkdir -p "$A"
cp Cookfile src.txt "$A/"

echo "=== cache_portability bug-hunt ==="
echo

echo "--- Step 1: fresh build at $A ---"
out_A1=$(cd "$A" && "$COOK" widget 2>&1 | grep -oE 'cook build done.*\)' | head -1)
echo "  fresh: $out_A1"

echo
echo "--- Step 2: rerun at $A (warm cache) ---"
out_A2=$(cd "$A" && "$COOK" widget 2>&1 | grep -oE 'cook build done.*\)' | head -1)
echo "  warm:  $out_A2"

if [[ "$out_A2" != *"1 cached recipes, 1 done"* ]]; then
    echo "FAIL — cache didn't even survive a same-path rerun"
    echo "  Cannot proceed with portability test."
    exit 1
fi

echo
echo "--- Step 3: cp -r $A → $B ---"
cp -r "$A" "$B"
ls "$B/.cook/cache/" 2>/dev/null && echo "  (cache files copied)"

echo
echo "--- Step 4: cook widget at $B ---"
out_B=$(cd "$B" && "$COOK" widget 2>&1 | grep -oE 'cook build done.*\)' | head -1)
echo "  moved: $out_B"

echo
if [[ "$out_B" == *"1 cached recipes, 1 done"* ]]; then
    echo "PASS — cache survived path move (§7.7 portability holds)"
    exit 0
else
    echo "FAIL — cache did not survive path move"
    echo "  expected: 1 cached recipes, 1 done"
    echo "  got:      $out_B"
    exit 1
fi
