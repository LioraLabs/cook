#!/bin/bash
# verify.sh — COOK-277 regression: canonical tool identity (CS-0157/CS-0158).
#
# A `tools { }` probe's sealed value is the binary's content hash only:
#   [relocated] identical bytes at a different PATH location keep the key —
#               pre-fix, the embedded path re-keyed every sealing unit and
#               heterogeneous fleets could never share sealed artifacts.
#   [upgraded]  changed bytes change the key — identity actually keys.
#   [readview]  `$<probe.NAME.path>` resolves freshly per run from the
#               metadata channel (the relocated run sees the NEW location).
#   [tools.id]  cook.tools.id folds a stable 64-hex identity into a custom
#               probe value.

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

STORE="$(mktemp -d)"
BIN_A="$(mktemp -d)"
BIN_B="$(mktemp -d)"
trap 'rm -rf "$STORE" "$BIN_A" "$BIN_B" .cook out.txt path.txt id.txt x.c; true' EXIT
mkdir -p .cook
printf '[cache]\ncache_dir = "%s"\n' "$STORE" > .cook/cloud.toml

# A tiny deterministic "toolchain": mycc IN OUT copies with a version stamp.
printf '#!/bin/sh\n# toolchain v1\ncat "$1" > "$2"\n' > "$BIN_A/mycc"
chmod +x "$BIN_A/mycc"
echo "source" > x.c

assert_result() {
    local name="$1" expected="$2"; shift 2
    scenario=$((scenario + 1))
    printf "  [%d] %-58s " "$scenario" "$name"
    local out
    out="$("$@" 2>&1 | grep -oE 'cook done.*\)' | head -1)"
    if [[ "$out" == *"$expected"* ]]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"
        echo "      expected substring: $expected"
        echo "      got:                $out"
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

echo "=== COOK-277: canonical tool identity ==="
echo

assert_result "cold build executes"    "0 cached recipes, 1 done" \
    env PATH="$BIN_A:$PATH" "$COOK" app
assert_true   "sealed value is hash-only (no path field)" \
    bash -c '! grep -q "\"path\"" .cook/probes/demo:tc.json && grep -q "\"hash\"" .cook/probes/demo:tc.json'

# [relocated] same bytes, different location, fresh local state.
cp "$BIN_A/mycc" "$BIN_B/mycc" && chmod +x "$BIN_B/mycc"
rm -rf .cook/cache .cook/probes out.txt
assert_result "relocated toolchain: shared-store HIT" "1 cached recipes, 1 done" \
    env PATH="$BIN_B:$PATH" "$COOK" app

# [readview] the path a consumer sees is the fresh location, not a cached one.
rm -f path.txt
env PATH="$BIN_B:$PATH" "$COOK" showpath >/dev/null 2>&1
assert_true   "read-view path is the CURRENT location" \
    grep -q "$BIN_B/mycc" path.txt

# [upgraded] byte change must re-key.
printf '#!/bin/sh\n# toolchain v2\ncat "$1" > "$2"\n' > "$BIN_B/mycc"
chmod +x "$BIN_B/mycc"
assert_result "upgraded toolchain bytes: re-executes" "0 cached recipes, 1 done" \
    env PATH="$BIN_B:$PATH" "$COOK" app

# [tools.id] custom probe folds a stable identity.
rm -f id.txt
env PATH="$BIN_B:$PATH" "$COOK" showid >/dev/null 2>&1
assert_true   "cook.tools.id yields a 64-hex identity" \
    bash -c 'grep -qE "^[0-9a-f]{64}$" id.txt'

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
