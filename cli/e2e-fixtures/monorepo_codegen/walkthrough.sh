#!/bin/bash
# walkthrough.sh — guided demo of cross-Cookfile caching in this workspace.
#
# Five scenarios, each asserting on cook's "(N nodes, X cached, Y done)" tail:
#   1. Fresh build               — full DAG executes
#   2. No-op rebuild             — full cache
#   3. Schema drift              — invalidates everything (diamond on proto)
#   4. Queue-only drift          — locality: cli stays cached
#   5. Subdir invocation         — .cookroot walk-up (§7.6 Rule 2)
#
# Set COOK= to override the cook binary location.

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
scenario=0

run_assert() {
    local name="$1"
    local expected="$2"
    shift 2
    scenario=$((scenario + 1))
    printf "  [%d] %-55s " "$scenario" "$name"
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
    find . -type d -name ".cook" -exec rm -rf {} + 2>/dev/null || true
    find . -type d -name "build" -exec rm -rf {} + 2>/dev/null || true
}

restore_schema() {
    cat > libs/proto/schema.proto <<'EOF'
message User { name: string; id: int }
message Job  { id: string; payload: bytes }
EOF
}

restore_queue_tmpl() {
    printf '[queue] topic=jobs durable=true\n' > libs/queue/queue.tmpl
}

echo "=== monorepo_codegen: cross-Cookfile caching walkthrough ==="
echo

echo "--- Scenario 1: fresh build (full DAG executes) ---"
restore_schema
restore_queue_tmpl
clean_state
run_assert "Fresh cook top" "0 cached recipes, 5 done" "$COOK" top

echo
echo "--- Scenario 2: no-op rebuild (full cache) ---"
run_assert "Re-run cook top hits cache for all 5" "5 cached recipes, 5 done" "$COOK" top

echo
echo "--- Scenario 3: schema.proto drift (diamond on proto invalidates everything) ---"
printf '\nmessage Drift { added: bool }\n' >> libs/proto/schema.proto
run_assert "schema drift rebuilds all 5" "0 cached recipes, 5 done" "$COOK" top
restore_schema

echo
echo "--- Scenario 4: queue.tmpl drift (locality: cli stays cached) ---"
clean_state
"$COOK" top >/dev/null 2>&1  # warm cache
printf 'retain_seconds=3600\n' >> libs/queue/queue.tmpl
run_assert "queue drift: 2 cached (proto, cli), 3 rebuilt" "2 cached recipes, 5 done" "$COOK" top
restore_queue_tmpl

echo
echo "--- Scenario 5: subdir invocation via .cookroot walk-up ---"
clean_state
scenario=$((scenario + 1))
printf "  [%d] %-55s " "$scenario" "cook queue_lib from libs/queue/ subdir"
sub_out=$(cd libs/queue && "$COOK" queue_lib 2>&1)
if echo "$sub_out" | grep -q '0 cached recipes, 2 done'; then
    echo "PASS"
    pass=$((pass + 1))
else
    echo "FAIL"
    echo "       got: $(echo "$sub_out" | grep -oE 'cook build done.*\)')"
    fail=$((fail + 1))
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
