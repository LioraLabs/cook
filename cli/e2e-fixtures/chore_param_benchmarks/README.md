# Chore Parameter Benchmarks

Verification fixture for **COOK-36** — chore parameters (positional, defaulted-string, Lua-expression-default, and variadic forms) plus the `@PRESET` / `--config` CLI argv-partitioning surface.

Each scenario in `verify.sh` exercises one slice of the new behavior and asserts on stdout (success cases) or stderr (error cases).

## Quick start

```sh
# Build cook
(cd ../../cli && cargo build --bin cook)

# Run every scenario
./verify.sh
```

Expected: **25 PASS, 0 FAIL.**

## The matrix

| # | Surface                                                  | Form demonstrated                                   |
|---|----------------------------------------------------------|-----------------------------------------------------|
| 1 | Required parameter                                       | `chore greet who` + `cook greet alice`              |
| 2 | Required missing                                         | `cook greet` → "requires parameter 'who'"           |
| 3 | Defaulted-string default fires                           | `chore deploy target host="prod.example.com"` + `cook deploy production` |
| 4 | Defaulted-string overridden                              | `cook deploy production myhost`                     |
| 5 | Execute-phase Lua sees both locals                       | `> print(... target .. host ...)` in `deploy`       |
| 6 | Lua-expression default fires                             | `version=(os.getenv("RELEASE_TAG") or "auto-vNULL")` |
| 7 | Lua-expression default reads env                         | `RELEASE_TAG=v3.2.1 cook release`                   |
| 8 | Variadic `+files` with one argv                          | `cook lint solo.lua`                                |
| 9 | Variadic `+files` preserves spaces (shell-quoted)        | `cook lint a.lua "b lua"` → `linting a.lua b lua`  |
| 10 | Variadic `+files` with zero argv errors                 | `cook lint` → "requires one or more values for variadic '+files'" |
| 11 | Variadic `*files` with zero argv binds `{}`             | `cook fmt` → `fmt #files=0`                         |
| 12 | Variadic `*files` with one argv                         | `cook fmt main.lua` → `#files=1, [1] main.lua`     |
| 13 | Comprehensive — register-phase Lua local (`>>`)         | `demo production myhost v1 a.lua b.lua`             |
| 14 | Comprehensive — shell `$<NAME>` substitution             | `shell-sub: production myhost v1 a.lua b.lua`       |
| 15 | Comprehensive — env-var export                          | `env-vars: target=production host=myhost ...`       |
| 16 | Comprehensive — variadic env-var space-joined           | `extras="a.lua b.lua"`                              |
| 17 | Comprehensive — execute-phase Lua sees prelude          | `exec-lua: ... #extras=2`                           |
| 18 | Comprehensive — defaults fire when argv exhausted       | `cook demo production` → all defaults fill in       |
| 19 | Config preset via `@sigil`                              | `cook demo production @release` → `mode=release`   |
| 20 | Config preset via `--config NAME`                       | `cook demo production --config release`             |
| 21 | Config preset via `-c NAME` short flag                  | `cook demo production -c release`                   |
| 22 | Two presets via sigil → error                           | `cook demo production @release @release`            |
| 23 | Mixed sigil + flag → error                              | `cook demo production @release --config release`    |
| 24 | `--` end-of-options separator                           | `cook demo -- @latest` → target=`@latest` (literal) |
| 25 | Migration-hint diagnostic for legacy `cook NAME PRESET` | `cook noop release` → "Did you mean a config preset? Use 'cook noop @release' or 'cook noop --config release'." |

## Why "benchmarks"?

Like `examples/cache_benchmarks` and `examples/test_benchmarks`, this fixture is a behavior-matrix scoreboard — each scenario is a single assertion-bearing invocation. The matrix's purpose is regression detection: any future refactor of the chore-parameter machinery (parser, codegen, runtime binding, placeholder expansion, env-var export, or CLI argv partitioning) must keep all 25 scenarios passing.

A scenario failure points to a specific surface: scenario 14 ➜ shell sigil expander; scenario 16 ➜ codegen env table; scenario 25 ➜ CLI partition + RegisterError mapping.

## Reading the Cookfile

The `Cookfile` declares six chores plus two config blocks. Each chore body uses one of three binding surfaces — Lua local (visible in `>>` and `>` steps), `$<NAME>` placeholder (in `@shell` steps with POSIX-safe quoting), or env var (exported into shell child processes). The comprehensive `demo` chore uses all three at once.

### A note on `$<NAME>` quoting

`$<NAME>` substitutes the bound value with POSIX-safe single-quote escaping. That makes it safe to pass values containing spaces or shell metacharacters as a single shell word — try `cook lint a.lua "b lua"` and watch each filename arrive at `printf` as one argument. The trade-off: if you write `@echo "literal $<who>"`, the single quotes become visible in the output. When you want a raw value pasted into a quoted string, read the env-var form (`$who`) inside double quotes instead.

The Standard cross-reference: §9 (placeholders), §7.1.1 (parameter forms), §7.1.2 (Lua-local binding + env-var export), §7.1.3 (variadic semantics), §13.2 (load-phase Lua expression evaluation), App. A.3.1 (grammar production).

## Adding a scenario

Append an `assert_stdout` or `assert_stderr_fail` line in `verify.sh`. Both helpers take a label, an expected substring, and the cook command + args (minus the binary path). Re-run `./verify.sh` to confirm the new scenario joins the 25.
