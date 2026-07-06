#!/usr/bin/env bash
# share-docker.sh — cross-machine cache sharing across TWO real containers.
#
# The same story as share-local.sh, made concrete: two separate containers
# (separate filesystems / local caches) share a Docker volume mounted at
# /shared, used as the Cache-trust v3 content-addressed store. A 'builder'
# container publishes `portable` by key; a 'consumer' container fetches it by
# key and does not re-run the command.
#
# The host's `cook` binary is bind-mounted into the containers, so the base
# image only needs a glibc recent enough to run it. Skips cleanly if docker is
# unavailable or the glibc is too old (override the image with COOK_DEMO_IMAGE).
set -uo pipefail
cd "$(dirname "$0")"
EX=$(pwd)
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"
[ -x "$COOK" ] || { echo "cook binary not found at $COOK — build it: (cd ../../cli && cargo build --bin cook)"; exit 1; }

docker info >/dev/null 2>&1 || { echo "SKIP: docker not available"; exit 0; }
IMG="${COOK_DEMO_IMAGE:-debian:trixie-slim}"   # needs glibc >= the binary's requirement
docker image inspect "$IMG" >/dev/null 2>&1 || docker pull "$IMG" >/dev/null 2>&1 \
  || { echo "SKIP: base image $IMG unavailable (set COOK_DEMO_IMAGE)"; exit 0; }

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT
SHARED="$ROOT/shared-store"; mkdir -p "$SHARED"   # bind-mounted into both containers as /shared
# Run as the host user so bind-mounted output is host-owned and cleanup just works.
USERSPEC="$(id -u):$(id -g)"

mk() { # $1 = machine dir
  mkdir -p "$1/.cook"
  printf '[cache]\ncache_dir = "/shared"\n' > "$1/.cook/cloud.toml"
  cp "$EX/Cookfile" "$EX/src.txt" "$1/"
}
A="$ROOT/builder"; B="$ROOT/consumer"; mk "$A"; mk "$B"

cook_in() { # $1=hostname $2=machine-dir $3=SIMHOST ; rest = cook args
  local hn="$1" pd="$2" sh="$3"; shift 3
  docker run --rm --hostname "$hn" --user "$USERSPEC" -e "SIMHOST=$sh" -e HOME=/tmp \
    -v "$COOK":/usr/local/bin/cook:ro -v "$pd":/work -v "$SHARED":/shared \
    -w /work "$IMG" /usr/local/bin/cook "$@"
}

# Pre-flight: confirm the binary actually runs in this image's glibc.
cook_in probe "$A" alpha --version >/dev/null 2>&1 \
  || { echo "SKIP: cook fails to run in $IMG (glibc too old?) — set COOK_DEMO_IMAGE"; exit 0; }

echo "=== Cache-trust v3: cross-machine sharing across two containers (shared store: $SHARED) ==="
fail=0

echo "--- container 'builder' (SIMHOST=alpha): cook portable ---"
cook_in builder "$A" alpha portable >/dev/null 2>&1
[ -f "$A/build/portable.txt" ] && echo "  PASS: builder produced build/portable.txt" || { echo "  FAIL: builder build"; fail=1; }

echo "--- container 'consumer' (different container, fresh local cache, same volume) ---"
why=$(cook_in consumer "$B" beta why portable 2>&1 | grep -oE '\[(HIT|MISS) \(shared\)\]' | head -1)
[ "$why" = "[HIT (shared)]" ] && echo "  PASS: consumer classifies portable as $why (content-addressed key)" \
                              || { echo "  FAIL: consumer why = '$why' (want HIT)"; fail=1; }
cook_in consumer "$B" beta portable >/dev/null 2>&1
if [ -f "$B/build/portable.txt" ] \
   && [ "$(sha256sum "$A/build/portable.txt" | cut -d' ' -f1)" = "$(sha256sum "$B/build/portable.txt" | cut -d' ' -f1)" ]; then
  echo "  PASS: consumer fetched the builder's exact bytes across containers (no rebuild)"
else echo "  FAIL: consumer output missing or differs"; fail=1; fi

echo
[ "$fail" = 0 ] && echo "=== container cache-sharing checks passed ===" || { echo "=== FAILURES ==="; exit 1; }
