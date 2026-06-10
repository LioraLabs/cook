#!/bin/bash
# verify.sh — assert the §8.4.2 `cook (LUA_EXPR)` output-expression example,
# two tiers:
#   1. codegen shape via `cook emit-lua` (parse + codegen, no execution).
#   2. execution: build both recipes, assert the rewritten subtree + sidecars,
#      and prove the one-to-one cache — edit one source and only its unit
#      rebuilds.
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
assert_contains "translate: expression evaluated per ingredient"   '_cook_out = (input:gsub("docs/en", "build/fr"))'
assert_contains "index: chained gsub expression"                   '_cook_out = (input:gsub("docs/en", "build/meta"):gsub("%.md$", ".json"))'
assert_contains "register-phase guard (Note 8.4.2.3 diagnostic)"   'cook (LUA_EXPR) returned non-string or empty value for input'
assert_contains "index: Lua body recorded as lua_code payload"     'lua_code = [['

# --- Tier 2: execution -------------------------------------------------------

echo
echo "execution assertions (cook translate / cook index):"
rm -rf build .cook

assert_true   "translate runs (path-rewrite fan-out)"    "$COOK" translate
assert_true   "index runs (Lua-body sidecars)"           "$COOK" index
assert_true   "nested subtree preserved (guide/)"        test -f build/fr/guide/intro.md
assert_file_eq "content transformed alongside the path"  build/fr/guide/setup.md \
"# Setup

Bonjour — install the tool, then come back."
assert_file_eq "sidecar carries the §23.1 input binding" build/meta/reference.json \
'{"source": "docs/en/reference.md", "lines": 3}'

# No-op rebuild: every per-ingredient unit is a cache hit.
RERUN="$("$COOK" translate 2>&1)"
assert_grep "clean re-run is fully cached"               "$RERUN" "3/3 cached"

# Edit ONE source file: only its unit re-runs (one-to-one over own inputs).
cp docs/en/guide/intro.md docs/en/guide/intro.md.bak
printf 'Hello again.\n' >> docs/en/guide/intro.md
EDIT="$("$COOK" translate 2>&1)"
mv docs/en/guide/intro.md.bak docs/en/guide/intro.md
assert_grep "edit one source → only its unit rebuilds"   "$EDIT" "2/3 cached"

# Leave the tree clean.
rm -rf build .cook

echo
echo "  $pass passed, $fail failed (of $n)"
[ "$fail" -eq 0 ] || exit 1
