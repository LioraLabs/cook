#!/bin/bash
# verify.sh — assert the §22.5.9 for_each-over-dependent-probe-chain example,
# two tiers:
#   1. codegen shape via `cook emit-lua` (parse + codegen, no execution).
#   2. execution (COOK-64): build, assert the chained outputs, and prove the
#      per-member cache survives a probe-chain re-evaluation — edit one service
#      and only its config rebuilds.
#
# Set COOK= to override the cook binary (default: workspace target).

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../../cli && cargo build --bin cook)"
    exit 1
fi

pass=0
fail=0
n=0

assert_contains() {  # <name> <substring> — searches the emitted Lua
    local name="$1"; local needle="$2"
    n=$((n + 1)); printf "  [%2d] %-58s " "$n" "$name"
    if printf '%s' "$LUA" | grep -qF -- "$needle"; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; echo "        expected to find: $needle"; fail=$((fail + 1))
    fi
}

assert_true() {  # <name> <command...>
    local name="$1"; shift
    n=$((n + 1)); printf "  [%2d] %-58s " "$n" "$name"
    if "$@" >/dev/null 2>&1; then echo "PASS"; pass=$((pass + 1)); else echo "FAIL"; fail=$((fail + 1)); fi
}

assert_file_eq() {  # <name> <path> <expected>
    local name="$1"; local path="$2"; local expected="$3"
    n=$((n + 1)); printf "  [%2d] %-58s " "$n" "$name"
    local got; got="$(cat "$path" 2>/dev/null)"
    if [ "$got" = "$expected" ]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; echo "        $path: expected [$expected], got [$got]"; fail=$((fail + 1))
    fi
}

assert_grep() {  # <name> <text> <pattern>
    local name="$1"; local text="$2"; local pat="$3"
    n=$((n + 1)); printf "  [%2d] %-58s " "$n" "$name"
    if printf '%s' "$text" | grep -q "$pat"; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; echo "        expected /$pat/ in: $text"; fail=$((fail + 1))
    fi
}

# --- Tier 1: codegen shape ---------------------------------------------------

LUA="$("$COOK" emit-lua 2>/dev/null)"
if [ -z "$LUA" ]; then echo "FAIL: cook emit-lua produced no output"; exit 1; fi

echo "codegen assertions (cook emit-lua):"
assert_contains "render: for_each source is the dependent probe" 'local _items = cook.cache.get("services")'
assert_contains "render: per-member loop binds item"             'for _, item in ipairs(_items) do'
assert_contains "render: \$<in.name> output path"              'tostring(item["name"])'
assert_contains "render: \$<in.url> in command"                'tostring(item["url"])'

# --- Tier 2: execution -------------------------------------------------------

echo
echo "execution assertions (cook render):"
rm -rf build .cook

assert_true   "render runs (probe chain → fan-out)"      "$COOK" render
assert_file_eq "auth.conf url derived through the chain" build/auth.conf \
"service  = auth
url      = http://localhost:8001
replicas = 3"
assert_file_eq "billing.conf rendered"                   build/billing.conf \
"service  = billing
url      = http://localhost:8002
replicas = 2"
assert_file_eq "search.conf rendered"                    build/search.conf \
"service  = search
url      = http://localhost:8003
replicas = 5"

# No-op rebuild: the whole chain re-evaluates but every member is a cache hit.
RERUN="$("$COOK" render 2>&1)"
assert_grep "clean re-run is fully cached"               "$RERUN" "3/3 cached"

# Edit ONLY auth's port in the upstream manifest. The chain re-evaluates
# (services_raw → services), but only auth's member changed.
cp data/services.json data/services.json.bak
sed -i 's/"8001"/"9001"/' data/services.json
EDIT="$("$COOK" render 2>&1)"
mv data/services.json.bak data/services.json
assert_grep "edit one service → only its unit rebuilds"  "$EDIT" "2/3 cached"
assert_file_eq "auth.conf reflects the edited port"      build/auth.conf \
"service  = auth
url      = http://localhost:9001
replicas = 3"
assert_file_eq "billing.conf unchanged (cache hit)"      build/billing.conf \
"service  = billing
url      = http://localhost:8002
replicas = 2"

# Leave the tree clean.
rm -rf build .cook

echo
echo "  $pass passed, $fail failed (of $n)"
[ "$fail" -eq 0 ] || exit 1
