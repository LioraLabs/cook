#!/bin/bash
# Directory-hopping tour: upward Cookfile discovery, cwd-scoped bare names,
# root-anchored cache keys, reserved `//` targets, `.cookroot` boundary.
# (Standard §20.2, book §10.1.)
#
# Each scenario asserts on cook's observable output. Exit status is the
# number of failing scenarios.

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

check() {
    local name="$1" ok="$2"
    scenario=$((scenario + 1))
    printf "  [%d] %-62s " "$scenario" "$name"
    if [ "$ok" = "yes" ]; then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL"
        fail=$((fail + 1))
    fi
}

clean_state() {
    find . -type d \( -name build -o -name .cook \) -exec rm -rf {} + 2>/dev/null || true
}

echo "=== directory_hopping tour ==="
echo

echo "--- 1. Build from the workspace root ---"
# Note: wiping .cook/ only clears local state — the shared content-addressed
# store may still warm this build, so don't pin the cached-recipe count.
clean_state
out=$("$COOK" build 2>&1)
ok=no; echo "$out" | grep -q '4 nodes.*4 done' && ok=yes
check "cook build at root (4 nodes, 4 done)" "$ok"
ok=no; grep -q 'directory hopping' build/bundle.txt 2>/dev/null && ok=yes
check "build/bundle.txt contains both members' output" "$ok"

echo
echo "--- 2. Bare names inside the member (apps/web) ---"
out=$(cd apps/web && "$COOK" build 2>&1)
ok=no; echo "$out" | grep -q '2 nodes, 2 cached recipes, 2 done' && ok=yes
check "cook build in apps/web (member's build, fully cached)" "$ok"
out=$(cd apps/web && "$COOK" theme.build 2>&1)
ok=no; echo "$out" | grep -q '1 cached recipes, 1 done' && ok=yes
check "cook theme.build in apps/web (own import visible)" "$ok"
(cd apps/web && "$COOK" web.build >/dev/null 2>&1)
rc=$?
ok=no; [ "$rc" -ne 0 ] && ok=yes
check "cook web.build in apps/web fails (root alias never leaks)" "$ok"
out=$(cd apps/web && "$COOK" menu 2>&1)
ok=no; [ "$out" = "$(printf '  recipe build\n  recipe theme.build')" ] && ok=yes
check "cook menu in apps/web shows the member view (no root alias)" "$ok"

echo
echo "--- 3. Upward discovery from a nested non-Cookfile dir ---"
out=$(cd apps/web/src && "$COOK" build 2>&1)
ok=no; echo "$out" | grep -q '2 nodes, 2 cached recipes, 2 done' && ok=yes
check "cook build in apps/web/src (walks up, still cached)" "$ok"

echo
echo "--- 4. Cache keys anchor at the workspace root ---"
root_keys=$("$COOK" why web.build --json 2>/dev/null | grep -oE '"(key|cache_key)": "[^"]*"' | sort)
member_keys=$(cd apps/web && "$COOK" why build --json 2>/dev/null | grep -oE '"(key|cache_key)": "[^"]*"' | sort)
nested_keys=$(cd apps/web/src && "$COOK" why build --json 2>/dev/null | grep -oE '"(key|cache_key)": "[^"]*"' | sort)
ok=no; [ -n "$root_keys" ] && [ "$root_keys" = "$member_keys" ] && [ "$member_keys" = "$nested_keys" ] && ok=yes
check "cook why keys identical from root / member / nested dir" "$ok"

echo
echo "--- 5. Reserved '//' targets ---"
out=$("$COOK" //check 2>&1)
rc=$?
ok=no; [ $rc -eq 1 ] && echo "$out" | grep -q 'reserved syntax' && ok=yes
check "cook //check exits 1 with reserved-syntax diagnostic" "$ok"

echo
echo "--- 6. The .cookroot boundary (decoy in a tmpdir) ---"
tmp=$(mktemp -d)
mkdir -p "$tmp/decoy/project/sub"
printf 'recipe build\n    @echo DECOY\n' > "$tmp/decoy/Cookfile"
touch "$tmp/decoy/project/.cookroot"
out=$(cd "$tmp/decoy/project/sub" && "$COOK" build 2>&1)
rc=$?
ok=no; [ $rc -ne 0 ] && echo "$out" | grep -q 'no Cookfile found' && echo "$out" | grep -q 'workspace boundary' && ok=yes
check "walk stops at .cookroot, decoy Cookfile above is never used" "$ok"
mkdir -p "$tmp/nowhere/deep"
out=$(cd "$tmp/nowhere/deep" && "$COOK" build 2>&1)
rc=$?
ok=no; [ $rc -ne 0 ] && echo "$out" | grep -q 'no Cookfile found.*filesystem root' && ok=yes
check "no Cookfile up to fs root fails with clear diagnostic" "$ok"
rm -rf "$tmp"

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
