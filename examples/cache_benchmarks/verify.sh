#!/bin/bash
# verify.sh — exercise the SHI-140 cache cloud-readiness scenarios + the
# 2026-05-02 addendum (per-output artifacts, restore-on-hit, monorepo).
#
# Each scenario asserts on cook's "(N nodes, X cached, Y done)" tail line.
# Prints PASS / FAIL per scenario. Exits non-zero on any failure.
#
# Set COOK= to override the cook binary location (default: workspace target).

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"

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
    rm -rf "$HOME/.cache/cook/cloud" 2>/dev/null || true
}

echo "=== SHI-140 cache cloud-readiness verification ==="
echo

echo "--- Scenarios 1-2: fresh build + no-op rebuild ---"
clean_state
run_assert "Fresh build executes 3 recipes"          "0 cached recipes, 3 done"  "$COOK" demo debug
run_assert "No-op rebuild hits all 3"                 "3 cached recipes, 3 done"  "$COOK" demo debug

echo
echo "--- Scenario 3: mtime touch (content-unchanged) ---"
touch src/greet.c
sleep 0.05  # ensure mtime resolution boundary
run_assert "Touch-only change still hits all 3"       "3 cached recipes, 3 done"  "$COOK" demo debug

echo
echo "--- Scenario 5-6: config CFLAGS toggle (variant coexistence + restore) ---"
# With the 2026-05-02 addendum:
# - greet+util have CFLAGS in their consulted env, so toggling produces
#   distinct cache entries per variant.
# - demo has NO CFLAGS dep, so its cache_key is the SAME across variants.
#   But its cache_meta.input_paths now includes greet.o/util.o (addendum §4.3),
#   so InputChanged fires when greet/util produce different bytes — demo rebuilds.
# - Toggling back to a prior variant: greet+util restore from artifact store
#   (addendum §5.2). demo still rebuilds because its single entry tracks
#   only the most-recent inputs.
clean_state
run_assert "Build under debug: all execute"           "0 cached recipes, 3 done"  "$COOK" demo debug
run_assert "Switch to release: all rebuild"           "0 cached recipes, 3 done"  "$COOK" demo release
run_assert "Toggle back to debug: greet+util restore" "2 cached recipes, 3 done"  "$COOK" demo debug
run_assert "Toggle to release: greet+util restore"    "2 cached recipes, 3 done"  "$COOK" demo release

echo
echo "--- Scenario 7: sister recipe insulation ---"
clean_state
run_assert "Sister fresh build"                       "0 cached recipes, 1 done"  "$COOK" sister
run_assert "Sister with debug: still hits"            "1 cached recipes, 1 done"  "$COOK" sister debug
run_assert "Sister with release: still hits"          "1 cached recipes, 1 done"  "$COOK" sister release

echo
echo "--- Scenario 8: denylisted env (HOME) ---"
"$COOK" demo debug >/dev/null 2>&1  # warm cache
run_assert "HOME change does not invalidate"          "3 cached recipes, 3 done"  env HOME=/some/other/path "$COOK" demo debug

echo
echo "--- Scenario 9: multi-output pair ---"
clean_state
run_assert "Multi-output pair: fresh execute"         "0 cached recipes, 1 done"  "$COOK" pair
run_assert "Multi-output pair: re-hit"                "1 cached recipes, 1 done"  "$COOK" pair

# 2026-05-02 addendum: every output uploads as its own artifact (per-output
# artifact_key). Verify both foo.txt and bar.txt have meta sidecars in the
# LocalBackend store.
echo
echo "--- Multi-output uploads N artifacts (addendum §5.1) ---"
foo_meta=$(find "$HOME/.cache/cook/cloud" -name "*.meta.json" 2>/dev/null \
    -exec grep -l '"output_path":"build/pair/foo.txt"' {} \; | head -1)
bar_meta=$(find "$HOME/.cache/cook/cloud" -name "*.meta.json" 2>/dev/null \
    -exec grep -l '"output_path":"build/pair/bar.txt"' {} \; | head -1)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "Both pair outputs uploaded as artifacts"
if [ -n "$foo_meta" ] && [ -n "$bar_meta" ]; then
    foo_size=$(grep -oE '"size_bytes":[0-9]+' "$foo_meta" | grep -oE '[0-9]+')
    bar_size=$(grep -oE '"size_bytes":[0-9]+' "$bar_meta" | grep -oE '[0-9]+')
    foo_disk=$(stat -c %s build/pair/foo.txt 2>/dev/null || stat -f %z build/pair/foo.txt)
    bar_disk=$(stat -c %s build/pair/bar.txt 2>/dev/null || stat -f %z build/pair/bar.txt)
    if [ "$foo_size" = "$foo_disk" ] && [ "$bar_size" = "$bar_disk" ]; then
        echo "PASS  (foo=$foo_size, bar=$bar_size)"
        pass=$((pass + 1))
    else
        echo "FAIL  (foo meta=$foo_size disk=$foo_disk, bar meta=$bar_size disk=$bar_disk)"
        fail=$((fail + 1))
    fi
else
    echo "FAIL  (foo_meta=$foo_meta bar_meta=$bar_meta)"
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 10: imported lib round-trip (monorepo, addendum §6) ---"
clean_state
run_assert "Mono fresh build (imports lib)"           "0 cached recipes, 5 done"  "$COOK" mono
run_assert "Mono no-op rebuild (4 cacheable hit)"     "4 cached recipes, 5 done"  "$COOK" mono

echo
echo "--- Scenario 11: cross-cookfile dep input drift ---"
# Touch lib/src/lib.c content to change its hash; lib_build invalidates.
# demo doesn't currently consume lib.lib_build's output (cross-cookfile
# body refs are a future work item per pipeline.rs:526), so demo still hits.
echo "// drift" >> lib/src/lib.c
out=$("$COOK" mono 2>&1)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "lib drift invalidates lib_build only"
if echo "$out" | grep -q '3 cached recipes, 5 done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got: $(echo "$out" | grep -oE 'cook build done.*\)')"
    fail=$((fail + 1))
fi
# Restore lib.c
git checkout -- lib/src/lib.c 2>/dev/null || true

echo
echo "--- Scenario 12: variant-toggle restore-on-hit (addendum §5.2) ---"
# Capture greet.o bytes after a debug build; toggle to release; toggle back to
# debug; restored greet.o bytes must match the original debug bytes.
clean_state
"$COOK" demo debug >/dev/null 2>&1
debug_o=$(sha256sum build/greet.o | awk '{print $1}')
"$COOK" demo release >/dev/null 2>&1
release_o=$(sha256sum build/greet.o | awk '{print $1}')
"$COOK" demo debug >/dev/null 2>&1
restored_o=$(sha256sum build/greet.o | awk '{print $1}')
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "Restored greet.o == original debug bytes"
if [ "$restored_o" = "$debug_o" ] && [ "$debug_o" != "$release_o" ]; then
    echo "PASS  (debug=${debug_o:0:8}, release=${release_o:0:8})"
    pass=$((pass + 1))
else
    echo "FAIL  (debug=${debug_o:0:8}, release=${release_o:0:8}, restored=${restored_o:0:8})"
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 13: multi-output round-trip restore (addendum §5.1) ---"
clean_state
"$COOK" pair >/dev/null 2>&1
foo_orig=$(sha256sum build/pair/foo.txt | awk '{print $1}')
bar_orig=$(sha256sum build/pair/bar.txt | awk '{print $1}')
# Wipe build/pair on disk; the cache entry remains and the artifacts are in
# the LocalBackend store. Re-running pair should restore both files.
rm -rf build/pair
"$COOK" pair >/dev/null 2>&1
foo_restored=$(sha256sum build/pair/foo.txt 2>/dev/null | awk '{print $1}')
bar_restored=$(sha256sum build/pair/bar.txt 2>/dev/null | awk '{print $1}')
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "Pair restores both outputs after wipe"
if [ "$foo_orig" = "$foo_restored" ] && [ "$bar_orig" = "$bar_restored" ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (foo $foo_orig→$foo_restored, bar $bar_orig→$bar_restored)"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
