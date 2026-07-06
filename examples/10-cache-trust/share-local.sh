#!/usr/bin/env bash
# share-local.sh — cross-"machine" cache sharing on ONE host.
#
# Two project checkouts ("machine A" and "machine B") with independent local
# caches point their `.cook/cloud.toml` cache_dir at the SAME shared store.
# This is the cross-machine half of the Cache-trust v3 story that the
# single-machine verify.sh can't show:
#
#   portable  (unannotated)  B reuses A's artifact by key — no rebuild
#   hostdep   `seal host`    B MISSES under a different host (probe re-keyed)
#   scratch   `local`        never reaches the shared store; B can't fetch it
#   generate  `record`       B reuses A's exact recording (non-reproducible value)
#   pin       `pinned`       a cold miss is a HARD ERROR, not a rebuild
#
# `cook why <recipe>` (read-only) classifies each unit as HIT/MISS against the
# shared store, which is the proof that the reuse is by content-addressed key.
set -uo pipefail
cd "$(dirname "$0")"
EX=$(pwd)
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"
[ -x "$COOK" ] || { echo "cook binary not found at $COOK — build it: (cd ../../cli && cargo build --bin cook)"; exit 1; }

ROOT=$(mktemp -d); trap 'rm -rf "$ROOT"' EXIT
SHARED="$ROOT/shared-store"; mkdir -p "$SHARED"   # the content-addressed store both machines share

mk() { # $1 = machine dir
  mkdir -p "$1/.cook"
  printf '[cache]\ncache_dir = "%s"\n' "$SHARED" > "$1/.cook/cloud.toml"
  cp "$EX/Cookfile" "$EX/src.txt" "$1/"
}
A="$ROOT/machine-A"; B="$ROOT/machine-B"; mk "$A"; mk "$B"
why() { ( cd "$2" && SIMHOST="$3" "$COOK" why "$1" 2>&1 | grep -oE '\[(HIT|MISS) \([a-z-]+\)\]' | head -1 ); }
fail=0; check() { if [ "$2" = "$3" ]; then echo "  PASS: $1"; else echo "  FAIL: $1 (got '$2', want '$3')"; fail=1; fi; }

echo "=== Cache-trust v3: cross-machine sharing (shared store: $SHARED) ==="

echo "--- Machine A (SIMHOST=alpha) publishes portable, hostdep, generate ---"
( cd "$A" && SIMHOST=alpha "$COOK" portable >/dev/null && SIMHOST=alpha "$COOK" hostdep >/dev/null \
            && SIMHOST=alpha "$COOK" generate >/dev/null && SIMHOST=alpha "$COOK" scratch >/dev/null ) \
  || { echo "FAIL: machine A build"; exit 1; }
genA=$(cat "$A/build/generated.txt")

echo "--- Machine B (SIMHOST=beta, fresh local cache, same store) classifies via 'cook why' ---"
check "portable: machine-independent -> HIT across the host change" "$(why portable "$B" beta)" "[HIT (shared)]"
check "hostdep: 'seal host' re-keys on host change   -> MISS"        "$(why hostdep  "$B" beta)" "[MISS (shared)]"
check "scratch: 'local' never published        -> MISS (local-only)" "$(why scratch  "$B" beta)" "[MISS (local-only)]"
check "generate: 'record' recording is shared        -> HIT"         "$(why generate "$B" beta)" "[HIT (shared)]"

echo "--- Machine B fetches the shared artifacts by key ---"
( cd "$B" && SIMHOST=beta "$COOK" generate >/dev/null )
genB=$(cat "$B/build/generated.txt")
if [ "$genA" = "$genB" ]; then echo "  PASS: B reused A's exact non-reproducible recording ($genB)"; else echo "  FAIL: B re-generated ($genA != $genB)"; fail=1; fi

echo "--- 'pinned' cold miss is a hard error (fresh machine C, EMPTY store) ---"
C="$ROOT/machine-C"; EMPTY="$ROOT/empty"; mkdir -p "$EMPTY" "$C/.cook"
printf '[cache]\ncache_dir = "%s"\n' "$EMPTY" > "$C/.cook/cloud.toml"
cp "$EX/Cookfile" "$EX/src.txt" "$C/"
if ( cd "$C" && "$COOK" pin >/dev/null 2>&1 ); then echo "  FAIL: pinned cold miss should have errored"; fail=1
else echo "  PASS: pinned cold miss aborted (exit $?), produced no output: $([ -f "$C/build/pinned.txt" ] && echo 'file exists (FAIL)' || echo none)"; fi

echo
[ "$fail" = 0 ] && echo "=== all cross-machine checks passed ===" || { echo "=== FAILURES ==="; exit 1; }
