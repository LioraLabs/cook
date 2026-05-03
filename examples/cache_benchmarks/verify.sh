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
    rm -rf "$HOME/.cache/cook/cloud" 2>/dev/null || true
}

echo "=== SHI-140 cache cloud-readiness verification ==="
echo

echo "--- Scenarios 1-2: fresh build + no-op rebuild ---"
# demo now depends on lib.lib_build via {lib.lib_build} body ref (§7.3),
# so `cook demo` runs 4 recipes: greet, util, lib.lib_build, demo.
clean_state
run_assert "Fresh build executes 4 recipes"          "0 cached recipes, 4 done"  "$COOK" demo debug
run_assert "No-op rebuild hits all 4"                 "4 cached recipes, 4 done"  "$COOK" demo debug

echo
echo "--- Scenario 3: mtime touch (content-unchanged) ---"
touch src/greet.c
sleep 0.05  # ensure mtime resolution boundary
run_assert "Touch-only change still hits all 4"       "4 cached recipes, 4 done"  "$COOK" demo debug

echo
echo "--- Scenario 5-6: config CFLAGS toggle (variant coexistence + restore) ---"
# With the 2026-05-02 addendum:
# - greet+util+lib.lib_build have CFLAGS in their consulted env, so toggling
#   produces distinct cache entries per variant.
# - demo's cache_key differs across variants via its dep input lib.lib_build
#   (§7.3 Phase-7 wiring): when lib.lib_build produces different bytes, demo
#   sees InputChanged on lib/build/lib.o and rebuilds.
# - Toggling back: greet+util+lib.lib_build restore from artifact store (3
#   cached). demo rebuilds because its stored entry tracks the release lib.o
#   hash, but the restored lib.o has debug bytes (InputChanged).
clean_state
run_assert "Build under debug: all execute"           "0 cached recipes, 4 done"  "$COOK" demo debug
run_assert "Switch to release: all rebuild"           "0 cached recipes, 4 done"  "$COOK" demo release
run_assert "Toggle back to debug: 3 restore+1 build"  "3 cached recipes, 4 done"  "$COOK" demo debug
run_assert "Toggle to release: 3 restore+1 build"     "3 cached recipes, 4 done"  "$COOK" demo release

echo
echo "--- Scenario 7: sister recipe insulation ---"
clean_state
run_assert "Sister fresh build"                       "0 cached recipes, 1 done"  "$COOK" sister
run_assert "Sister with debug: still hits"            "1 cached recipes, 1 done"  "$COOK" sister debug
run_assert "Sister with release: still hits"          "1 cached recipes, 1 done"  "$COOK" sister release

echo
echo "--- Scenario 8: denylisted env (HOME) ---"
"$COOK" demo debug >/dev/null 2>&1  # warm cache
run_assert "HOME change does not invalidate"          "4 cached recipes, 4 done"  env HOME=/some/other/path "$COOK" demo debug

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
# Change lib/src/lib.c content (add a real function, not a comment) so the
# compiled output changes hash. lib_build invalidates; demo also rebuilds
# because it consumes lib.lib_build via {lib.lib_build} body ref (§7.3).
# greet and util stay cached (2 cached recipes, 5 done).
printf '\nvoid lib_drift_sentinel(void) { }\n' >> lib/src/lib.c
out=$("$COOK" mono 2>&1)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "lib drift invalidates lib_build and demo"
if echo "$out" | grep -q '2 cached recipes, 5 done'; then
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
echo "--- Scenario 14: {lib.lib_build} body subst — cache_meta.input_paths (Phase 7 wiring) ---"
# demo references {lib.lib_build} in its body (§7.3). After Phase 7, this
# lowers to cook.dep_output("lib.lib_build"). The importer-relative path
# lib/build/lib.o lands in cache_meta.input_paths. Proof: demo caches on
# the second invocation when lib.c hasn't changed.
clean_state
run_assert "demo first build: all 4 execute"         "0 cached recipes, 4 done"  "$COOK" demo
run_assert "demo re-run: all 4 hit cache"            "4 cached recipes, 4 done"  "$COOK" demo

echo
echo "--- Scenario 15: dep input drift across import ---"
# Adding a real function to lib.c changes the compiled lib.o hash. lib_build
# rebuilds (new lib.o), and demo also rebuilds because its cache_meta.input_paths
# includes lib/build/lib.o whose content has changed (InputChanged). greet
# and util stay cached (2 cached recipes, 4 done).
clean_state
"$COOK" demo >/dev/null 2>&1
printf '\nvoid lib_drift_s15(void) { }\n' >> lib/src/lib.c
out=$("$COOK" demo 2>&1)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "lib content change → lib_build+demo rebuild"
if echo "$out" | grep -q '2 cached recipes, 4 done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got: $(echo "$out" | grep -oE 'cook build done.*\)')"
    fail=$((fail + 1))
fi
git checkout -- lib/src/lib.c 2>/dev/null || true

echo
echo "--- Scenario 16: workspace-root inference from deep subdir ---"
# Running `cook lib_build` from within lib/ should succeed. The workspace-root
# inference (Rule 3: walk up and find a Cookfile that tree-imports this dir)
# locates the parent as root. The cache file lands at lib/.cook/cache/lib_build.bin.
clean_state
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "cook lib_build from lib/ subdir succeeds"
if (cd lib && "$COOK" lib_build >/dev/null 2>&1); then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (cook lib_build from lib/ returned non-zero)"
    fail=$((fail + 1))
fi
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "lib_build.bin created in lib/.cook/cache"
if [ -f lib/.cook/cache/lib_build.bin ]; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (lib/.cook/cache/lib_build.bin missing)"
    fail=$((fail + 1))
fi
# Second run from lib/ should hit cache.
sub_out=$((cd lib && "$COOK" lib_build 2>&1) || true)
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" "lib_build second run hits cache from lib/"
if echo "$sub_out" | grep -q '1 cached recipes, 1 done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got: $(echo "$sub_out" | grep -oE 'cook build done.*\)')"
    fail=$((fail + 1))
fi

echo
echo "--- Scenario 17: .cookroot marker overrides inference ---"
# Placing a .cookroot marker at the workspace root should still allow the
# same build to succeed (Rule 2: .cookroot walk-up takes precedence).
# Behavior is identical to without .cookroot.
clean_state
"$COOK" demo >/dev/null 2>&1  # warm cache before marker test
touch .cookroot
run_assert "demo with .cookroot still caches (4 hit)" "4 cached recipes, 4 done"  "$COOK" demo
scenario=$((scenario + 1))
printf "  [%2d] %-55s " "$scenario" ".cookroot at root does not break subdir invocation"
if (cd lib && "$COOK" lib_build >/dev/null 2>&1); then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL  (cook lib_build from lib/ failed with .cookroot present)"
    fail=$((fail + 1))
fi
rm -f .cookroot

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
