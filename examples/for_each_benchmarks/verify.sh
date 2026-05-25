#!/bin/bash
# verify.sh — assert the §8.3 for_each codegen shape of every recipe.
#
# COOK-63 lands the parser + codegen; the register-time runtime (probe pre-pass,
# $(cmd) capture, member rendering) is COOK-64. So this verifies through the
# transpiler (`cook emit-lua`), which runs parse + codegen without executing.
# When COOK-64 lands, add an execution tier (run the recipes, assert outputs).
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

LUA="$("$COOK" emit-lua 2>/dev/null)"
if [ -z "$LUA" ]; then
    echo "FAIL: cook emit-lua produced no output"
    exit 1
fi

pass=0
fail=0
n=0

# assert_contains <name> <substring>
assert_contains() {
    local name="$1"; local needle="$2"
    n=$((n + 1))
    printf "  [%2d] %-62s " "$n" "$name"
    if printf '%s' "$LUA" | grep -qF -- "$needle"; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; echo "        expected to find: $needle"; fail=$((fail + 1))
    fi
}

echo "for_each codegen assertions (cook emit-lua):"

# cards_cook — probe source, cook fan-out, $<item.FIELD>.
assert_contains "cards_cook: probe member source"      'local _items = cook.cache.get("cards")'
assert_contains "cards_cook: per-member loop binds item" 'for _, item in ipairs(_items) do'
assert_contains "cards_cook: \$<item.id> output"        'tostring(item["id"])'
assert_contains "cards_cook: \$<item.name> in command"  'tostring(item["name"])'

# catalog_cook — probe key:field, bare $<item>.
assert_contains "catalog_cook: key:field indexes array" 'cook.cache.get("catalog")["items"]'
assert_contains "catalog_cook: bare \$<item> renders member" 'cook.member_to_string(item)'

# deploy — $(cmd) default JSON capture, plate fan-out.
assert_contains "deploy: \$(cmd) shell capture"         'cook.sh("cat data/hosts.ndjson")'
assert_contains "deploy: default source JSON-decodes"   'table.insert(_items, cook.json_decode(_line))'
assert_contains "deploy: \$<item.host> in plate body"   'tostring(item["host"])'
assert_contains "deploy: plate emits add_unit"          'cook.add_unit({command ='

# render — $(cmd) as lines, raw members.
assert_contains "render: ls capture"                    'cook.sh("ls posts")'
assert_contains "render: as lines keeps raw member"     'table.insert(_items, _line)'

# eval — probe source, test fan-out.
assert_contains "eval: cases probe source"              'local _items = cook.cache.get("cases")'
assert_contains "eval: \$<item.input> in test body"     'tostring(item["input"])'
assert_contains "eval: test emits add_test"             'cook.add_test({command ='

echo
echo "  $pass passed, $fail failed (of $n)"
[ "$fail" -eq 0 ] || exit 1
