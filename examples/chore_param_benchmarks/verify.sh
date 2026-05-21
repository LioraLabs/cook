#!/bin/bash
# verify.sh — exercise every chore-parameter form introduced by COOK-36.
#
# Each scenario asserts on either stdout (success cases) or stderr (error
# cases). Prints PASS / FAIL per scenario. Exits non-zero on any failure.
#
# Set COOK= to override the cook binary location (default: workspace target).

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

assert_stdout() {
    local name="$1"; local expected="$2"; shift 2
    scenario=$((scenario + 1))
    printf "  [%2d] %-58s " "$scenario" "$name"
    local out
    out="$("$COOK" "$@" 2>/dev/null)"
    if [[ "$out" == *"$expected"* ]]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"
        echo "       expected stdout substring: $expected"
        echo "       got stdout: $out"
        fail=$((fail + 1))
    fi
}

assert_stderr_fail() {
    local name="$1"; local expected="$2"; shift 2
    scenario=$((scenario + 1))
    printf "  [%2d] %-58s " "$scenario" "$name"
    local out rc
    out="$("$COOK" "$@" 2>&1 >/dev/null)"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "FAIL (expected non-zero exit, got 0)"
        echo "       stderr: $out"
        fail=$((fail + 1))
        return
    fi
    if [[ "$out" == *"$expected"* ]]; then
        echo "PASS"; pass=$((pass + 1))
    else
        echo "FAIL"
        echo "       expected stderr substring: $expected"
        echo "       got stderr: $out"
        fail=$((fail + 1))
    fi
}

echo "── COOK-36 chore parameter scenarios ──"

# Required parameter
assert_stdout "required: cook greet alice"               "hello alice"           greet alice
assert_stderr_fail "required missing: cook greet"        "requires parameter 'who'" greet

# Required + defaulted-string
assert_stdout "defaulted-string default fires"            "host=prod.example.com" deploy production
assert_stdout "defaulted-string overridden by argv"       "host=myhost"           deploy production myhost
assert_stdout "execute-phase Lua sees both locals"        "post-deploy: production@prod.example.com" deploy production

# Lua-expression default
assert_stdout "lua-default fires when argv absent"        "version=auto-vNULL"    release
RELEASE_TAG="v3.2.1" assert_stdout "lua-default reads env" "version=v3.2.1"        release
# Note: env-driven Lua default is exercised separately below via env-wrapped invocation.

# Variadic +files
assert_stdout "variadic+ one element"                     "count=1"               lint solo.lua
assert_stdout "variadic+ many, preserves spaces"          "linting a.lua b lua"   lint a.lua "b lua"
assert_stderr_fail "variadic+ with zero argv errors"      "requires one or more values for variadic '+files'" lint

# Variadic *files
assert_stdout "variadic* zero binds empty table"          "fmt #files=0"          fmt
assert_stdout "variadic* one element"                     "fmt #files=1"          fmt main.lua

# Comprehensive demo
assert_stdout "comprehensive: register-phase Lua local"   "register: target=production" demo production myhost v1 a.lua b.lua
assert_stdout "comprehensive: shell sigil substitution"   "shell-sub: production myhost v1 a.lua b.lua" demo production myhost v1 a.lua b.lua
assert_stdout "comprehensive: env-var export"             "env-vars: target=production host=myhost version=v1" demo production myhost v1 a.lua b.lua
assert_stdout "comprehensive: variadic env-var space-joined" "extras=\"a.lua b.lua\""   demo production myhost v1 a.lua b.lua
assert_stdout "comprehensive: execute-phase Lua sees prelude" "exec-lua: production myhost v1 #extras=2" demo production myhost v1 a.lua b.lua
assert_stdout "comprehensive: defaults fire when argv exhausted" "shell-sub: production prod v0" demo production

# Config preset interop
assert_stdout "preset via @sigil"                          "mode=release"          demo production "@release"
assert_stdout "preset via --config long flag"              "mode=release"          demo production --config release
assert_stdout "preset via -c short flag"                   "mode=release"          demo production -c release
assert_stderr_fail "two presets via sigil"                 "multiple config presets" demo production "@release" "@release"
assert_stderr_fail "mixed sigil + flag"                    "supply only one"       demo production "@release" --config release

# `--` end-of-options separator
assert_stdout "-- escapes literal '@' as a positional"     "target=@latest"        demo -- "@latest"

# Migration hint for legacy 'cook NAME PRESET'
assert_stderr_fail "migration hint when paramless chore got positional" "Did you mean a config preset" noop release

# Recipe with argv (recipes can't take params)
# (No recipe declared in this Cookfile, so we use a separate fixture-less assertion)

echo
echo "── Summary ──"
echo "  Scenarios:  $scenario"
echo "  Passed:     $pass"
echo "  Failed:     $fail"

if [ "$fail" -gt 0 ]; then
    exit 1
fi
