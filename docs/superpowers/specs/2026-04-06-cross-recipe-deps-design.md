# Cross-Recipe Dependency Inference

**Linear ticket**: SHI-114
**Date**: 2026-04-06

## Summary

Infer cross-recipe unit-level dependencies from `{recipe_name}` references in cook/plate/test steps, enabling fine-grained parallelism within a single DAG while preserving the existing wave execution model for explicit `: dep` dependencies.

## Definitions

**Terminal output**: The output file(s) produced by a recipe's final `cook` step. If the final step is OneToOne, the terminal output is the list of files it produced (one per input). If ManyToOne, it is the single output file. If a recipe has no `cook` steps, it has no terminal output. Terminal outputs are ordered by registration order (the order `cook.add_unit()` was called during Lua execution).

## The `{dep}` Reference System

Recipes reference other recipes' terminal outputs using `{recipe_name}` syntax in cook/plate/test steps.

### Two forms

| Form | Where valid | Meaning |
|---|---|---|
| `{dep}` | Command | Terminal output(s) of dep, space-joined string |
| `{dep.stem/name/ext/dir}` | Output pattern | Drives OneToOne iteration over dep's terminal outputs |

### Existing placeholders (unchanged)

`{in}`, `{out}`, `{stem}`, `{name}`, `{ext}`, `{dir}`, `{all}` always refer to the current recipe's own pipeline. When iteration is dep-driven (`{dep.stem}` in output pattern), `{in}` points to the current dep output item. `{all}` always refers to the previous internal cook step's outputs.

### Examples

String substitution in command (no iteration):
```
recipe app
    ingredients "src/main.c"
    cook "build/obj/main.o" using "gcc -c {in} -o {out}"
    cook "build/bin/app" using "gcc -o {out} {in} {libmath} {libstr}"
end
```

Dep-driven iteration (no `ingredients` needed -- `{protos.stem}` in the output pattern drives iteration over `protos`' terminal outputs, so `{in}` is the current proto output being iterated):
```
recipe compile_protos
    cook "build/obj/{protos.stem}.o" using "gcc -c {in} -o {out}"
    cook "build/lib/libprotos.a" using "ar rcs {out} {all}"
end
```

Mixed (iteration from dep, string substitution from another dep):
```
recipe server
    cook "build/obj/{protos.stem}.o" using "gcc -c {in} -I{core}/include -o {out}"
end
```

### Rules

- One iteration source per step. Mixing `{stem}` and `{dep.stem}` in an output pattern is an error.
- Multiple dep iteration sources in one output pattern is an error.
- `{dep}` in an output pattern is an error (it doesn't drive iteration).
- `{dep}` string substitution in a command alongside dep-driven iteration in the output pattern is fine.
- Plate and test steps follow the same rules as cook steps. The only difference is their outputs don't feed the next step's `{in}`/`{all}` chain.
- No `{dep.all}` or `{dep.in}` accessors. `{dep}` covers "all terminal outputs" and `{in}` covers "current iteration item" regardless of source.

## Disambiguation

### Recipe name vs environment variable

A pre-scan pass extracts all recipe names from all Cookfiles (including workspace imports) before codegen. The pre-scan collects fully-qualified recipe names (e.g., `backend.libcore`). `{dep}` references resolve against this full set. At template expansion, recipe names take precedence over environment variables. If a recipe name shadows an env var, emit a warning.

This disambiguation applies to ALL template expansion paths -- cook/plate/test commands, output patterns, and bare shell steps.

### Dotted names

Split on the last dot. If the suffix is a known accessor (`stem`, `name`, `ext`, `dir`), treat it as `recipe.accessor`. Otherwise treat the entire string as a namespaced recipe name.

- `{backend.build}` -- last segment `build` is not an accessor -- recipe name `backend.build`
- `{protos.stem}` -- last segment `stem` is an accessor -- recipe `protos`, accessor `stem`

### Reserved recipe name segments

A recipe's final name segment cannot be: `stem`, `name`, `ext`, `dir`, `in`, `out`, `all`. Validated at parse time.

## Two Dependency Mechanisms

| Mechanism | Syntax | Effect |
|---|---|---|
| Explicit dep | `recipe B: A` | Wave boundary -- A fully executes before B registers |
| Inferred dep | `{A}` in B's steps | Same-wave merge -- A and B register together in one DAG |

- Transitive `{dep}` chains collapse into one wave. If A references `{B}` and B references `{C}`, all three merge into one wave.
- `{dep}` merging respects existing wave boundaries. If recipe A is constrained to wave 2 (because of `: dep` on something in wave 1), then a recipe using `{A}` joins wave 2 -- it never pulls A to an earlier wave.
- Using both `: A` and `{A}` for the same dependency emits a warning (conflicting scheduling intent -- `: A` requests a wave boundary, `{A}` requests same-wave merging).
- Cycle detection happens at recipe level during pre-scan. Within-recipe steps are linear, so recipe-level cycles are the only possible kind.

### Example wave grouping

```
recipe libmath
    cook "build/lib/libmath.a" ...
end
recipe libstr
    cook "build/lib/libstr.a" ...
end
recipe app
    cook ... using "gcc ... {libmath} {libstr}"
end
recipe run: app
    test "build/bin/app"
end
```

- Wave 1: `libmath` + `libstr` + `app` -- one DAG, app's link unit waits on archive units, compiles run in parallel across all three recipes.
- Wave 2: `run` -- starts after wave 1 finishes (`: app` is a wave boundary).

## Engine Phases (revised)

1. **Parse** -- extract recipes, their steps, and `{recipe_name}` references from all Cookfiles.
2. **Build wave graph** -- `: dep` edges define wave boundaries; `{dep}` edges define intra-wave groups.
3. **Per wave:**
   a. Register all recipes in the wave in toposorted order (ordered by `{dep}` edges).
   b. Each recipe stores its terminal outputs; downstream recipes resolve `{dep}` via `cook.dep_output("name")` in Lua at registration time.
   c. Build one DAG for the entire wave with fine-grained unit-level edges: the unit containing `{dep}` depends on the specific unit(s) producing dep's terminal output.
   d. Execute the DAG with maximal parallelism.
4. **Next wave** (repeat from 3).

## Fine-Grained Unit-Level Wiring

The current `dag_builder.rs` wires cross-recipe edges coarsely: root units of B depend on all leaf units of A. The new model is precise: only the unit whose command contains `{dep}` depends on the unit(s) producing dep's terminal output.

This means B's compile steps can start immediately while A is still building. Only the step that actually consumes A's output waits.

To implement this, `cook.dep_output("libmath")` calls during registration record which unit is being built when the call happens, creating the fine-grained edge data.

## Lua-Side Resolution

At codegen time, the template expander emits Lua expressions for dep references:

- `{dep}` in command -- `cook.dep_output("dep")` (returns space-joined terminal outputs)
- `{dep.stem}` in output pattern -- triggers dep-driven iteration codegen, calling `cook.dep_output("dep")` to get the list and iterating with path extraction

These resolve at registration time. Since recipes register in toposorted order within a wave, the dependency's outputs are always available when needed.

## Error Conditions

| Condition | When detected | Severity |
|---|---|---|
| `{dep}` references unknown recipe | Pre-scan | Error |
| `{dep}` references recipe with no terminal output | Registration | Error |
| Multiple iteration sources in one output pattern | Codegen | Error |
| Recipe name collides with reserved accessor | Parse | Error |
| Cycle in `{dep}` graph | Pre-scan | Error |
| Recipe name shadows env var | Parse | Warning |
| Both `: dep` and `{dep}` for same dependency | Pre-scan | Warning |

## What Changes

| Component | Change |
|---|---|
| Template expander (`template.rs`) | Recognize `{recipe_name}` and `{recipe_name.accessor}`, emit `cook.dep_output()` calls |
| Parser/pre-scan | Extract `{recipe_name}` references, validate reserved names, detect cycles |
| `cook_step.rs` | New codegen path for dep-driven iteration |
| `dag_builder.rs` | Fine-grained cross-recipe edges from dep_output tracking, not just DepKind |
| `run.rs` | Wave grouping logic: `: dep` = wave boundary, `{dep}` = same-wave merge |
| `unit_api.rs` | `cook.dep_output()` Lua function, terminal output storage per recipe |
| `recipe_dag.rs` | Adapted for two-tier grouping (waves from `: dep`, intra-wave toposort from `{dep}`) |

## What Stays the Same

| Component | Status |
|---|---|
| `cook`/`plate`/`test` syntax | Unchanged |
| `cook.add_unit()` API | Unchanged -- already accepts inputs/output |
| `cook.export()`/`cook.import()` | Coexists -- used for arbitrary data sharing, not scheduling |
| Explicit `: dep` syntax | Still works as wave boundaries |
| `cook-dag` crate | Generic DAG unchanged, just receives different edges |
| Cache subsystem | Unchanged -- `{dep}` resolves to file paths at registration time, so the command string changes if dep's outputs change, naturally invalidating the cache |
