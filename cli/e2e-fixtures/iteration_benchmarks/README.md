# CS-0022 Iteration Benchmarks

Concrete, runnable proof that CS-0022's unified iteration model does what it
promises. Every mode from §4.5.1 of the Cook Standard is exercised by a
named recipe; the `benchmarks` orchestrator runs them all.

## Overview

CS-0022 replaces Cook's previous split iteration model with a single rule: a
`cook` step's **iteration mode is determined entirely by its output pattern
list**, before the `using` clause is consulted. Three modes result:

| Output pattern shape | Mode | Units produced |
|---|---|---|
| At least one output contains `{in.ACCESSOR}` | One-to-one over own inputs | One per ingredient |
| At least one output contains `{dep.ACCESSOR}` | One-to-one over dep outputs | One per dep output |
| All outputs are literal (no accessor) | Many-to-one | Exactly one |

A multi-output step whose patterns all carry `{in.ACCESSOR}` is "one-to-many"
(N inputs → N units × M outputs each). The mode is orthogonal to the declared
output count.

## The eight modes

| Recipe | Mode | Output pattern | Units (N=8) | Outputs per unit |
|---|---|---|---|---|
| `one_to_one_shell` | One-to-one, shell | `build/one_to_one_shell/{in.stem}.out` | 8 | 1 |
| `one_to_one_lua` | One-to-one, Lua | `build/one_to_one_lua/{in.stem}.out` | 8 | 1 |
| `one_to_many_shell` | One-to-many, shell | `build/one_to_many_shell/{in.stem}.a.out` + `.b.out` | 8 | 2 |
| `many_to_one_shell` | Many-to-one, shell | `build/many_to_one_shell/all.out` (literal) | 1 | 1 |
| `many_to_one_lua` | Many-to-one, Lua (wart-fix) | `build/many_to_one_lua/all.out` (literal) | 1 | 1 |
| `many_to_one_multi_shell` | Many-to-one, multi-output, shell | `build/.../all.a.out` + `all.b.out` (literal) | 1 | 2 |
| `many_to_one_multi_lua` | Many-to-one, multi-output, Lua | `build/.../all.a.out` + `all.b.out` (literal) | 1 | 2 |
| `dep_driven` | Dep-driven (one-to-one over `stage1` outputs) | `build/dep_driven/{stage1.stem}.final` | 8 | 1 |

Supporting recipes:

| Recipe | Role |
|---|---|
| `stage1` | One-to-one shell; provides the driver for `dep_driven` |
| `libfoo` | Many-to-one shell; produces a single library output |
| `app_with_lib` | Cross-recipe reference: `{libfoo}` inside `using { }` |
| `benchmarks` | Orchestrator: depends on all of the above |

## Running

Run from this directory. Cook uses the `Cookfile` in the current directory.

```sh
# Run a single mode
cook one_to_one_shell
cook many_to_one_lua
cook dep_driven

# Run all modes via the orchestrator
cook benchmarks

# Override sleep duration (default: 0.5s)
cook --set SLEEP=0.1 benchmarks

# Wipe build/ and .cook cache
cook clean
```

## Observing parallelism

The `-j` flag sets the number of parallel workers (default: number of CPU
cores). Set `SLEEP` to 0.5s (the default) and compare `-j 1` vs `-j 8`:

```sh
cook clean
time cook -j 8 benchmarks --output=plain  # parallel

cook clean
time cook -j 1 benchmarks --output=plain  # serial
```

### Measured timings (this machine, SLEEP=0.5, N=8 inputs)

```
-j 8:  cook build done in 3.03s
-j 1:  cook build done in 23.15s
```

Wall-clock ratio: **7.6×**. The serial run takes ~7.6× longer because the
eight iterating recipes each produce 8 units that must all run sequentially,
while the parallel run fans them out across workers.

Breakdown for `-j 8`:

- Iterating recipes (`one_to_one_*`, `one_to_many_*`, `stage1`, `dep_driven`):
  each produces 8 units. With 8 workers all 8 units run in one 0.5s wave.
- Aggregate recipes (`many_to_one_*`, `libfoo`, `app_with_lib`, `benchmarks`):
  each produces 1 unit. These interleave with the iterating recipes.
- Total elapsed: ~3s (six 0.5s waves, accounting for the stage1 → dep_driven
  dependency chain).

## Observing the DAG

The `dag` subcommand opens an interactive DAG viewer in your browser:

```sh
cook dag benchmarks
# cook: DAG viewer at http://127.0.0.1:<port>
# press Ctrl+C to stop
```

To see the work units and the generated Lua without executing:

```sh
cook emit-lua benchmarks | head -80
cook menu
```

The `menu` subcommand lists every recipe with its ingredient patterns, output patterns, and
declared dependencies — a quick structural overview.

## Verifying the wart-fix (CS-0022 §3.5)

Pre-CS-0022, a single-output Lua block always iterated per input — one unit per
ingredient, `inputs` inside the body was always a singleton. CS-0022 fixes
this: iteration is owned by the output pattern. A literal output pattern
(`"build/many_to_one_lua/all.out"`) means exactly one unit; `inputs` inside
the body is the full ingredient list.

After running `cook many_to_one_lua` (or `cook benchmarks`):

```
cat build/many_to_one_lua/all.out
```

Expected output (N=8 inputs):

```
many_to_one_lua: 8 inputs
  inputs[1] = src/inputs/01.txt
  inputs[2] = src/inputs/02.txt
  inputs[3] = src/inputs/03.txt
  inputs[4] = src/inputs/04.txt
  inputs[5] = src/inputs/05.txt
  inputs[6] = src/inputs/06.txt
  inputs[7] = src/inputs/07.txt
  inputs[8] = src/inputs/08.txt
```

The canonical CS-0022 regression check: if `#inputs == 1` here, the wart is
back. `#inputs == 8` confirms the fix is in effect.

## Second-stage and test modes

CS-0024 added shared iteration-mode detection for un-sandboxed side-effect
steps and `test` steps: the body content (not the output pattern)
determines whether each step runs once per item, once for all items, or
once unconditionally. CS-0135 removed the un-sandboxed `plate` step itself
— the shapes below now run as a second, declared-output `cook` step
(cached, and still one-to-one/many-to-one/one-shot per its own output
pattern), kept here purely for iteration-mode benchmark coverage.

### Mode detection for cook/test bodies

| Body contains | Mode | Units |
|---|---|---|
| `$<in>` or `$<in.X>` (shell) / `input` (Lua) | One-to-one | One per cook output |
| collected list (shell) / `inputs` (Lua) | Many-to-one | Exactly one |
| Neither | One-shot | Exactly one |

### Second-stage recipes (six modes, formerly `plate`)

| Recipe | Body form | Mode | What it does |
|---|---|---|---|
| `plate_one_to_one_shell` | `$<in>` in shell | One-to-one | Writes a `.plated` file per cook output |
| `plate_many_to_one_shell` | `inputs` in Lua | Many-to-one | Writes a report listing all cook outputs |
| `plate_one_shot_shell` | no source ref | One-shot | Writes a `.flag` file once |
| `plate_one_to_one_lua` | `input` in Lua | One-to-one | Writes a `.plated` file per cook output |
| `plate_many_to_one_lua` | `inputs` in Lua | Many-to-one | Writes a report with the input count |
| `plate_one_shot_lua` | no source ref | One-shot | Touches a `.flag` file once |

### Test recipes (six modes)

| Recipe | Body form | Test mode | Assertion |
|---|---|---|---|
| `test_one_to_one_shell` | `{in}` in shell | One-to-one | `test -f {in}` — each cook output exists |
| `test_many_to_one_shell` | `{all}` in shell | Many-to-one | word-count of `{all}` > 0 |
| `test_one_shot_shell` | no source ref | One-shot | `true` — always passes |
| `test_one_to_one_lua` | `input` in Lua | One-to-one | `io.open(input)` succeeds |
| `test_many_to_one_lua` | `inputs` in Lua | Many-to-one | `#inputs > 0` |
| `test_one_shot_lua` | no source ref | One-shot | `assert(true)` |

## SLEEP tuning

`SLEEP` defaults to `0.5` seconds (set in the `config` block). Override it
with `--set` to speed up or slow down the benchmark:

```sh
cook --set SLEEP=0.1 -j 4 benchmarks   # fast
cook --set SLEEP=2.0 -j 1 one_to_one_shell  # exaggerate serial gap
```
