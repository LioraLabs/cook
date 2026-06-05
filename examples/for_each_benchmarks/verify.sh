#!/bin/bash
# verify.sh — assert §8.3 for_each, two tiers:
#   1. codegen shape via `cook emit-lua` (parse + codegen, no execution).
#   2. execution (COOK-64): run every recipe, assert outputs, and prove the
#      §22.5.9 / §17.1 per-member cache — editing one member re-runs only its
#      unit while the rest stay cache hits.
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

# cards_cook — probe source, cook fan-out, $<in.FIELD>.
assert_contains "cards_cook: probe member source"      'local _items = cook.cache.get("cards")'
assert_contains "cards_cook: per-member loop binds item" 'for _, item in ipairs(_items) do'
assert_contains "cards_cook: \$<in.id> output"        'tostring(item["id"])'
assert_contains "cards_cook: \$<in.name> in command"  'tostring(item["name"])'

# catalog_cook — probe key:field, bare $<in>.
assert_contains "catalog_cook: key:field indexes array" 'cook.cache.get("catalog")["items"]'
assert_contains "catalog_cook: bare \$<in> renders member" 'cook.member_to_string(item)'

# deploy — $(cmd) default JSON capture, plate fan-out.
assert_contains "deploy: \$(cmd) shell capture"         'cook.sh("cat data/hosts.ndjson")'
assert_contains "deploy: default source JSON-decodes"   'table.insert(_items, cook.json_decode(_line))'
assert_contains "deploy: \$<in.host> in plate body"   'tostring(item["host"])'
assert_contains "deploy: plate emits add_unit"          'cook.add_unit({command ='

# render — $(cmd) as lines, raw members.
assert_contains "render: ls capture"                    'cook.sh("ls posts")'
assert_contains "render: as lines keeps raw member"     'table.insert(_items, _line)'

# eval — probe source, test fan-out.
assert_contains "eval: cases probe source"              'local _items = cook.cache.get("cases")'
assert_contains "eval: \$<in.input> in test body"     'tostring(item["input"])'
assert_contains "eval: test emits add_test"             'cook.add_test({command ='

# --- Tier 2: execution (COOK-64 runtime) ------------------------------------

# assert_true <name> <command...>  — runs the command, PASS on exit 0.
assert_true() {
    local name="$1"; shift
    n=$((n + 1))
    printf "  [%2d] %-62s " "$n" "$name"
    if "$@" >/dev/null 2>&1; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; fail=$((fail + 1))
    fi
}

# assert_file_eq <name> <path> <expected-content>
assert_file_eq() {
    local name="$1"; local path="$2"; local expected="$3"
    n=$((n + 1))
    printf "  [%2d] %-62s " "$n" "$name"
    local got; got="$(cat "$path" 2>/dev/null)"
    if [ "$got" = "$expected" ]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"; echo "        $path: expected [$expected], got [$got]"; fail=$((fail + 1))
    fi
}

echo
echo "for_each execution assertions (cook <recipe>):"

# Clean slate: wipe local build + cache so the first run is a real miss.
rm -rf build .cook

# Each source form runs end-to-end and lands its declared outputs.
assert_true   "cards_cook runs (probe → cook)"          "$COOK" cards_cook
assert_file_eq "cards_cook: ace.txt content"           build/cards/ace.txt  "Ace of Spades"
assert_file_eq "cards_cook: queen.txt content"          build/cards/queen.txt "Queen of Hearts"
assert_true   "catalog_cook runs (probe:field → cook)"  "$COOK" catalog_cook
assert_file_eq "catalog_cook: bare \$<in> is JSON"    build/catalog/widget.json '{"id":"widget","name":"Widget"}'
assert_true   "deploy runs (\$(cmd) NDJSON → plate)"     "$COOK" deploy
assert_true   "render runs (\$(cmd) as lines → cook)"    "$COOK" render
assert_file_eq "render: raw-member output"              build/html/intro.md.html '<article>intro.md</article>'
assert_true   "eval runs (probe → test)"                "$COOK" eval

# Per-member cache (§22.5.9 / §17.1 observable #5): a no-op re-run is fully
# cached; editing ONE member re-runs only that member's unit.
RERUN="$("$COOK" cards_cook 2>&1)"
n=$((n + 1)); printf "  [%2d] %-62s " "$n" "cards_cook: clean re-run is fully cached"
if printf '%s' "$RERUN" | grep -q "3/3 cached"; then echo "PASS"; pass=$((pass + 1)); else echo "FAIL"; fail=$((fail + 1)); fi

# Edit only the king card; ace must stay cached, king must re-run.
cp data/cards.json data/cards.json.bak
sed -i 's/Queen of Hearts/Queen of Spades/' data/cards.json
EDIT="$("$COOK" cards_cook 2>&1)"
mv data/cards.json.bak data/cards.json
n=$((n + 1)); printf "  [%2d] %-62s " "$n" "cards_cook: edit one member → 2/3 cached"
if printf '%s' "$EDIT" | grep -q "2/3 cached"; then echo "PASS"; pass=$((pass + 1)); else echo "FAIL"; echo "        re-run output: $EDIT"; fail=$((fail + 1)); fi
assert_file_eq "cards_cook: queen.txt reflects the edit"  build/cards/queen.txt "Queen of Spades"
assert_file_eq "cards_cook: ace.txt unchanged (cache hit)" build/cards/ace.txt "Ace of Spades"

# Stale-output reconciliation (§17.7 / CS-0093): dropping a data member sweeps
# its orphaned output on the next run, while live members are retained.
"$COOK" cards_cook >/dev/null 2>&1
n=$((n + 1)); printf "  [%2d] %-62s " "$n" "cards_cook: jack.txt present before drop"
if [ -f build/cards/jack.txt ]; then echo "PASS"; pass=$((pass + 1)); else echo "FAIL"; fail=$((fail + 1)); fi
cp data/cards.json data/cards.json.bak
printf '[\n  {"id": "ace",  "name": "Ace of Spades"},\n  {"id": "queen",  "name": "Queen of Hearts"}\n]\n' > data/cards.json
SWEEP="$("$COOK" cards_cook 2>&1)"
mv data/cards.json.bak data/cards.json
n=$((n + 1)); printf "  [%2d] %-62s " "$n" "cards_cook: dropped member's output swept"
if [ ! -f build/cards/jack.txt ]; then echo "PASS"; pass=$((pass + 1)); else echo "FAIL"; echo "        build/cards/jack.txt should have been swept; run output: $SWEEP"; fail=$((fail + 1)); fi
assert_file_eq "cards_cook: surviving member ace.txt retained" build/cards/ace.txt "Ace of Spades"

# Leave the tree clean.
rm -rf build .cook

echo
echo "  $pass passed, $fail failed (of $n)"
[ "$fail" -eq 0 ] || exit 1
