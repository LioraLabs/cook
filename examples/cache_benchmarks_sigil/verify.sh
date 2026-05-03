#!/bin/bash
# verify.sh — exercise the cache_benchmarks_sigil fixture.
#
# Covers:
#   - // (sigil) workspace-root-anchored import resolution
#   - {core.core_lib} cross-Cookfile body reference (Phase 7 wiring)
#   - dep input tracking: core.o change invalidates top
#   - subdir invocation via .cookroot walk-up (Rules 2 & 3)
#
# Each scenario asserts on cook's "(N nodes, X cached, Y done)" tail line.
# Prints PASS / FAIL per scenario. Exits non-zero on any failure.
#
# Set COOK= to override the cook binary location (default: workspace target).

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
# Resolve to absolute path so subshell `cd subdir && "$COOK" ...` works.
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../../cli && cargo build --bin cook)"
    exit 1
fi

pass=0
fail=0
scenario=0

run_assert() {
    local name="$1"
    local expected="$2"
    shift 2
    scenario=$((scenario + 1))
    printf "  [%2d] %-55s " "$scenario" "$name"
    local out
    out="$("$@" 2>&1 | grep -oE 'cook build done.*\)' | head -1)"
    if [[ "$out" == *"$expected"* ]]; then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL"
        echo "       expected substring: $expected"
        echo "       got:                $out"
        fail=$((fail + 1))
    fi
}

clean_state() {
    "$COOK" clean >/dev/null 2>&1 || true
    find . -type d -name ".cook" -exec rm -rf {} + 2>/dev/null || true
    find . -type d -name "build" -exec rm -rf {} + 2>/dev/null || true
    rm -rf "$HOME/.cache/cook/cloud" 2>/dev/null || true
}

restore_core_c() {
    printf 'int core_value(void) { return 42; }\n' > core/lib/src/core.c
}

echo "=== cache_benchmarks_sigil: // sigil import + cross-Cookfile body ref ==="
echo

echo "--- Scenarios 1-2: fresh build + no-op rebuild ---"
# top depends on core.core_lib (sigil import) and web.web_obj (tree import).
# First build: all 3 recipes execute. Second build: all 3 hit cache.
restore_core_c
clean_state
run_assert "Fresh build: all 3 recipes execute"      "0 cached recipes, 3 done"  "$COOK" top
run_assert "No-op rebuild: all 3 hit cache"           "3 cached recipes, 3 done"  "$COOK" top

echo
echo "--- Scenario 3: mtime touch (content-unchanged) ---"
# Touching core.c without changing content must still hit cache (content-addressed).
touch core/lib/src/core.c
sleep 0.05  # ensure mtime resolution boundary
run_assert "Touch-only change still hits all 3"       "3 cached recipes, 3 done"  "$COOK" top

echo
echo "--- Scenario 4: sigil dep input drift (core.c content change) ---"
# Adding a real function to core.c changes core.o hash. core_lib rebuilds.
# top also rebuilds because it body-refs {core.core_lib} — the rewritten dep
# path core/lib/build/core.o lands in top's cache_meta.input_paths (Phase 7
# wiring). web_obj has no dependency on core and stays cached (1 cached).
restore_core_c
clean_state
"$COOK" top >/dev/null 2>&1  # warm cache
printf '\nint core_drift(void) { return 1; }\n' >> core/lib/src/core.c
out=$("$COOK" top 2>&1)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "core.c drift → core_lib+top rebuild, web_obj cached"
if echo "$out" | grep -q '1 cached recipes, 3 done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got: $(echo "$out" | grep -oE 'cook build done.*\)')"
    fail=$((fail + 1))
fi
restore_core_c

echo
echo "--- Scenario 5-7: subdir invocation via .cookroot walk-up ---"
# Running cook from a subdirectory of the workspace root. The .cookroot marker
# at the workspace root is found by walk-up (Rule 2), enabling single-recipe
# builds from deep subdirectories.
clean_state
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "cook core_lib from core/lib/ subdir succeeds"
if (cd core/lib && "$COOK" core_lib >/dev/null 2>&1); then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (cook core_lib from core/lib/ returned non-zero)"
    fail=$((fail + 1))
fi
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "core_lib.bin created in core/lib/.cook/cache"
if [ -f core/lib/.cook/cache/core_lib.bin ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (core/lib/.cook/cache/core_lib.bin missing)"
    fail=$((fail + 1))
fi
sub_out=$((cd core/lib && "$COOK" core_lib 2>&1) || true)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "core_lib second run hits cache from core/lib/"
if echo "$sub_out" | grep -q '1 cached recipes, 1 done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got: $(echo "$sub_out" | grep -oE 'cook build done.*\)')"
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 8: web subdir invocation via .cookroot ---"
# apps/web/ is two levels deep from the workspace root. .cookroot walk-up
# should still resolve correctly.
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "cook web_obj from apps/web/ subdir succeeds"
if (cd apps/web && "$COOK" web_obj >/dev/null 2>&1); then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (cook web_obj from apps/web/ returned non-zero)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
