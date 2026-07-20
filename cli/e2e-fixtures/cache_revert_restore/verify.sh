#!/bin/bash
# verify.sh — COOK-278 regression: edit-then-revert restores the cached
# artifact instead of re-executing, for discovered-input (depfile) units.
#
# Scenario A (`chunks`): glob outputs with CONTENT-DEPENDENT concrete names
#   (the Next.js hashed-chunk shape). Pre-fix, the revert re-executed because
#   the restore probed the last run's stale concrete names.
# Scenario B (`sets`): the discovered SET changes with input content.
#   Pre-fix, the revert re-executed because the single-set manifest was
#   last-writer-wins and the older set was erased.
#
# Both must settle to a local skip afterwards (fat StepEntry recording), and
# scenario A must sweep the edited build's stale chunk on restore.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../.. && cargo build --bin cook)"
    exit 1
fi

pass=0
fail=0
scenario=0

# Hermetic store: this fixture must not read or pollute ~/.cache/cook/cloud.
STORE="$(mktemp -d)"
trap 'rm -rf "$STORE" .cook build chunks.d sets.d out.txt main.src header.h header_a.h header_b.h cloud_backup.toml 2>/dev/null; true' EXIT
mkdir -p .cook
printf '[cache]\ncache_dir = "%s"\n' "$STORE" > .cook/cloud.toml

assert_cached() {
    local name="$1"; shift
    scenario=$((scenario + 1))
    printf "  [%d] %-58s " "$scenario" "$name"
    local out
    out="$("$@" 2>&1 | grep -oE 'cook done.*\)' | head -1)"
    if [[ "$out" == *"1 cached recipes, 1 done"* ]]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"
        echo "      expected: 1 cached recipes, 1 done (restore, no re-execution)"
        echo "      got:      $out"
        fail=$((fail + 1))
    fi
}

assert_executed() {
    local name="$1"; shift
    scenario=$((scenario + 1))
    printf "  [%d] %-58s " "$scenario" "$name"
    local out
    out="$("$@" 2>&1 | grep -oE 'cook done.*\)' | head -1)"
    if [[ "$out" == *"0 cached recipes, 1 done"* ]]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"
        echo "      expected: 0 cached recipes, 1 done (genuine execution)"
        echo "      got:      $out"
        fail=$((fail + 1))
    fi
}

assert_true() {
    local name="$1"; shift
    scenario=$((scenario + 1))
    printf "  [%d] %-58s " "$scenario" "$name"
    if "$@"; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; fail=$((fail + 1))
    fi
}

echo "=== COOK-278: revert restores instead of re-executing ==="
echo
echo "--- Scenario A: content-dependent output names under a glob ---"

echo "main line" > main.src
echo "original header" > header.h

assert_executed "cold build executes"                    "$COOK" chunks
echo "edited header" > header.h
assert_executed "edit re-executes"                       "$COOK" chunks
EDITED_CHUNK="$(ls build/)"
echo "original header" > header.h
assert_cached   "revert RESTORES (was: full re-execute)" "$COOK" chunks
assert_true     "original chunk content restored" \
    grep -q "original header" build/chunk-*.txt
assert_true     "stale edited chunk swept" \
    test ! -e "build/$EDITED_CHUNK" -o "$(ls build | wc -l)" = "1"
assert_cached   "settled run is a cache hit"             "$COOK" chunks
echo "edited header" > header.h
assert_cached   "re-edit RESTORES the edited artifact"   "$COOK" chunks

echo
echo "--- Scenario B: discovered SET changes with content ---"

echo "header a v1" > header_a.h
echo "header b" > header_b.h

assert_executed "cold build executes (set = {header_a})" "$COOK" sets
echo "include_b now" > header_a.h
assert_executed "edit re-executes (set gains header_b)"  "$COOK" sets
echo "header a v1" > header_a.h
assert_cached   "revert RESTORES via the older set"      "$COOK" sets
assert_true     "restored output matches the original" \
    grep -q "header a v1" out.txt
assert_cached   "settled run is a cache hit"             "$COOK" sets

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
