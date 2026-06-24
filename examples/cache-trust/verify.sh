#!/usr/bin/env bash
# cache-trust — exercises the Cache-trust v3 single-key model end-to-end.
#
#   portable  (unannotated) shares fleet-wide; HITS across a host change
#   hostdep   `seal host`   re-keys + rebuilds on a host change
#   scratch   `local`       never published to the shared store
#   generate  `record`      warm hit reuses the recording (no re-generate)
set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"
[ -x "$COOK" ] || { echo "cook binary not found at $COOK"; exit 1; }

WD=$(mktemp -d); CACHE=$(mktemp -d)
trap 'rm -rf "$WD" "$CACHE"' EXIT
cp Cookfile src.txt "$WD/"
mkdir -p "$WD/.cook"
printf '[cache]\ncache_dir = "%s"\n' "$CACHE" > "$WD/.cook/cloud.toml"
cd "$WD"

echo "=== cache-trust ==="
echo "--- cold build (SIMHOST=alpha) ---"
SIMHOST=alpha "$COOK" build >/dev/null || { echo "FAIL: cold build"; exit 1; }
SIMHOST=alpha "$COOK" generate >/dev/null || { echo "FAIL: cold generate"; exit 1; }
gen1=$(cat build/generated.txt)

echo "--- warm rerun (SIMHOST=alpha): record warm hit ---"
SIMHOST=alpha "$COOK" generate >/dev/null || { echo "FAIL: warm generate"; exit 1; }
gen2=$(cat build/generated.txt)
[ "$gen1" = "$gen2" ] && echo "PASS: record warm hit reused recording ($gen1)" \
    || { echo "FAIL: record re-generated ($gen1 -> $gen2)"; exit 1; }

echo "--- host change (SIMHOST=beta) ---"
hp1=$(cat build/hostdep.txt); pp1=$(cat build/portable.txt)
SIMHOST=beta "$COOK" build >/dev/null || { echo "FAIL: rebuild on beta"; exit 1; }
hp2=$(cat build/hostdep.txt); pp2=$(cat build/portable.txt)
[ "$pp1" = "$pp2" ] && echo "PASS: portable HIT across host change" \
    || { echo "FAIL: portable rebuilt across host change"; exit 1; }
[ "$hp1" != "$hp2" ] && echo "PASS: hostdep re-keyed + rebuilt on host change" \
    || { echo "FAIL: hostdep did not rebuild on host change"; exit 1; }

echo "--- sharing: scratch (local) is never published ---"
if grep -rql '\[scratch\]' "$CACHE" 2>/dev/null; then
    echo "FAIL: a local unit leaked into the shared store"; exit 1
else
    echo "PASS: local unit stayed off the shared store"
fi
echo "=== all checks passed ==="
