# Design: Cook-step iteration unification (`{in.X}` accessor surface, single-form using-clause)

**Date:** 2026-04-30
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Scope:** Cook Standard (chapters §4, §5, §6.7, App. A, App. B), the Rust parser (`cli/crates/cook-lang`), the codegen (`cli/crates/cook-luagen`), `tree-sitter-cook`, and conformance fixtures.

## 1. Motivation

The Cook Standard at v0.4 has accumulated three closely-related but underspecified surfaces around the `cook` step:

1. **Iteration mode is split across two normative sources.** §6.7 says "if the using-string contains `{in}`, the step is one-to-one." §5.4 says "if the output pattern contains `{lib.ACCESSOR}`, the step iterates over `lib`'s outputs." Two rules, two anchors, with overlapping scope and silent footguns when an author writes `cook "out" using "do {in}"` (literal output + `{in}`-driven iteration → N units clobber the same output).

2. **Shell blocks are second-class.** §6.7 normatively defines `{in}`/`{out}`/`{stem}`/etc. only for the single-string `using "cmd"` form. The block forms `using {…}` and `using >{…}` get one informative note (§6.7 N.1) saying they "expose the same iteration directly as Lua or shell text" — but the Standard never specifies a mechanism for shell blocks. The reference implementation in `cli/crates/cook-luagen/src/cook_step.rs:170-183` confirms the gap: shell-block content is passed verbatim to the worker with no placeholder substitution. An author writing

    ```cook
    cook "build/parser.so" using {
        tree-sitter build . -o {out}
    }
    ```

    creates a literal file named `{out}` on disk.

3. **`using "cmd"` and `using {…}` overlap.** A single-line shell command can be written `using "cc {in} -o {out}"` or `using { cc {in} -o {out} }`; with placeholders specified for the former and unspecified for the latter, authors must memorize "use the string form for one-liners, the block form for multi-line." The string/block split has no semantic value once shell blocks are first-class.

4. **The Lua-block iteration model has a wart.** Today (`cook_step.rs:71-126`), a single-output Lua block always iterates per input, even when the output pattern has no accessor; a multi-output Lua block always runs once. There is no clean way to write a many-to-one Lua block (e.g., "concatenate all `.o` into one `.a` from Lua"). Authors hit this wall and route around it with a one-output sentinel or fall back to the shell form.

5. **Cross-recipe references stop at the block boundary.** §5.5 specifies `{lib}` substitution in "using-string, plate command, test command, or bare shell." Shell blocks and Lua blocks are not in scope. So `cook "app" using { gcc -o {out} main.c {libmath} }` silently does not substitute `{libmath}`.

The cumulative effect is that the surface has **two ways to iterate**, **two ways to write a shell command**, **one way that doesn't iterate when it should**, **one way that iterates when it shouldn't**, and **one cross-recipe reference rule that stops at the brace**. This design unifies all five into a single mental model, removes the redundant surface, and pushes iteration into one normative location.

## 2. Non-goals

- **Persistent shell sessions across multiple shell-block units.** Each unit's shell block remains a fresh `/bin/sh -c` process; this design does not introduce shared cwd, env, or shell state across units.
- **New path accessors.** The set `{stem, name, ext, dir}` is preserved verbatim. No `{base}`, `{abs}`, etc.
- **A `{lib_N}` indexed cross-recipe form.** Cross-recipe references stay at "bare `{lib}` = full list" granularity; per-element access goes through Lua.
- **Migration tooling.** v0.4 is pre-release lockstep posture (see `MEMORY.md` → "Cook Standard governs language changes"). Existing fixtures and example Cookfiles update in the same change set.
- **A general expression language inside `{ … }`.** Placeholders remain `{NAME}` or `{NAME.ACCESSOR}`. No nested expressions, no arithmetic, no string functions.
- **Lua block textual `{lib}` substitution.** §B.6.1's existing rationale (Lua has direct binding; shell does not) holds. Lua blocks continue to use `cook.dep_output()` / `cook.dep_output_list()`.

## 3. Design

### 3.1. The single iteration rule

A new subsection in §4.5 (or a renumber-creating §4.5.1 "Iteration mode") becomes the **sole** normative source for cook-step iteration. §6.7's "using-string `{in}` triggers iteration" rule is removed. §5.4 is preserved but reframed as an instance of the single rule.

**Rule.** A cook step's **iteration mode** is determined entirely by its output pattern list, before the `using` clause is consulted:

| Output pattern shape | Mode | Driver | Units produced |
|---|---|---|---|
| At least one output contains `{in.ACCESSOR}` (own-input accessor) | **One-to-one over own inputs** | The step's resolved `ingredients` list (§4.4) | One per input |
| At least one output contains `{lib.ACCESSOR}` (dep-driven accessor) | **One-to-one over dep outputs** | `lib`'s output list (§5.4.1) | One per `lib` output |
| All outputs are literal (no accessor placeholders) | **Many-to-one** (also called "non-iterating" — the "one" refers to *one unit*, not one output) | None — one unit runs with the full input list visible | Exactly one |

The mode determines **how many work units the step produces**; it is orthogonal to the cook step's **declared output count**. Either iteration mode can declare 1 or N outputs:

- Single-output one-to-one: `cook "{in.stem}.o"` — N inputs ⇒ N units, each producing 1 output.
- Multi-output one-to-one ("one-to-many"): `cook "{in.stem}.js" "{in.stem}.wasm"` — N inputs ⇒ N units, each producing 2 outputs.
- Single-output many-to-one: `cook "build/app"` — any number of inputs ⇒ 1 unit, producing 1 output.
- Multi-output many-to-one: `cook "out.js" "out.wasm"` — any number of inputs ⇒ 1 unit, producing 2 outputs.

A conforming implementation MUST report a load-time error for a step whose outputs mix iteration sources — e.g., one output bears `{in.stem}` and another bears `{libmath.stem}`, or one is `{in.stem}.o` and another is the literal `final.bin`. All output patterns of a single cook step share one driver.

The previous §6.7 paragraph that classified a step by the contents of its using-string is deleted. A using-string `{in}` in a step whose output pattern is literal becomes a load-time error (§3.3 below specifies the diagnostics).

### 3.2. The using-clause collapses to two forms

The `using_clause` grammar (App. A.4) becomes:

```ebnf
using_clause       ::= "using" ( shell_block | using_lua_block )
block_using_clause ::= "using" ( shell_block | using_lua_block )
```

The single-string form `using "cmd"` is **removed**. A shell block written on one line — `using { cc {in} -o {out} }` — is a valid `shell_block` per §2.9 (lexical brace-blocks); the existing brace-balance lexer already accepts a single-line block. No new grammatical form is introduced.

The `block_using_clause` (multi-output rule, §4.6) collapses to the same two forms; the multi-output normative text in §4.6 simplifies.

### 3.3. Placeholder vocabulary inside a using-clause body

A new normative table replaces §6.7's "shell using-string placeholders." The same table governs **both** shell blocks (textual register-time substitution) and Lua blocks (where applicable — see §3.5). Bare path-accessors (`{stem}`, `{name}`, `{ext}`, `{dir}`) are removed from the placeholder vocabulary.

| Placeholder | Valid in mode | Meaning |
|---|---|---|
| `{in}` | one-to-one (own or dep-driven) | The current iteration item (path) |
| `{in.ACCESSOR}` | one-to-one | `path.ACCESSOR(in)` per §6.6 |
| `{out}` | any mode, single output | The unit's single output path |
| `{out.ACCESSOR}` | any mode, single output | `path.ACCESSOR(out)` |
| `{out_N}` (N ∈ 1..) | any mode, multi-output | The unit's Nth declared output, in declaration order |
| `{out_N.ACCESSOR}` | any mode, multi-output | `path.ACCESSOR(out_N)` |
| `{all}` | many-to-one only | The unit's input list, space-joined |
| `{lib}` | any mode | Recipe `lib`'s full output list, space-joined (§5.5) |
| `{lib.ACCESSOR}` | **rejected in using-clause** | use `{in.ACCESSOR}` if `lib` is the driver; reach for Lua otherwise (firewall preserved from §5.4) |
| `{TOKEN}` (none of the above, not a recipe name) | any mode | `cook.env[TOKEN]` per §5.2 step 4 |

A conforming implementation MUST reject:
- `{in}` or `{in.X}` in a many-to-one step (no current iteration);
- `{all}` in a one-to-one step (no batched input list);
- A bare path-accessor (`{stem}`, `{name}`, `{ext}`, `{dir}`) anywhere — these were shorthand for the current driver and now read as undefined `{TOKEN}` resolution targets, which §5.2 step 4 sends to `cook.env[TOKEN]`. Authors writing `{stem}` in v0.4 surface code expecting v0.3 behavior get an env-lookup, not an iteration accessor — that is the wrong failure mode. The implementation MUST emit a specific diagnostic for the four bare path-accessor names that names the new form (`{in.stem}` etc.).
- `{out}` in a multi-output step (ambiguous — use `{out_N}`);
- `{out_N}` in a single-output step (use plain `{out}`);
- `{out_N}` for `N` greater than the declared output count.

`{out.ACCESSOR}` and `{out_N.ACCESSOR}` are admitted because composing path accessors on the unit's output is a common need (`mkdir -p {out.dir}`); they cost nothing once `{NAME.ACCESSOR}` is the canonical form.

### 3.4. Output-pattern surface

The same `{in.ACCESSOR}` / `{lib.ACCESSOR}` syntax governs the output pattern. §5.4's existing accessor placeholder for dep-driven iteration is preserved verbatim. Own-input iteration, formerly triggered by bare `{stem}` etc. in the output pattern, is re-spelled as `{in.ACCESSOR}`. The output pattern thus serves as the *declaration site* for both the iteration source and (when applicable) the per-iteration filename shape:

```cook
recipe build
    ingredients "src/*.c"
    cook "build/{in.stem}.o" using {
        gcc -c {in} -o {out}
    }
```

The output pattern's `{in.stem}` simultaneously says "iterate over own ingredients" and "the per-iteration output filename is `build/<stem of input>.o`." There is no separate iteration-trigger keyword; the pattern's accessor placeholder is the trigger.

Multi-output coherence (§3.1's last paragraph) follows naturally: all output patterns of one step must agree on the driver. `cook "{in.stem}.js" "{in.stem}.wasm" using {…}` is a one-to-many step (one input → two outputs per iteration); `cook "out.js" "out.wasm" using {…}` is a many-to-one step with two literal outputs.

### 3.4.1. Worked examples — the four mode/output combinations

The single iteration rule produces four legal cook-step shapes. The placeholder vocabulary in §3.3 covers each one:

```cook
# (1) Single-output one-to-one — N inputs ⇒ N units, each producing 1 output.
recipe compile
    ingredients "src/*.c"
    cook "build/{in.stem}.o" using {
        gcc -c {in} -o {out}
    }

# (2) Multi-output one-to-one ("one-to-many") — N inputs ⇒ N units, each producing 2 outputs.
recipe wasm_each
    ingredients "src/*.rs"
    cook "build/{in.stem}.js" "build/{in.stem}.wasm" using {
        wasm-pack build {in}
        cp pkg/main.js {out_1}
        cp pkg/main.wasm {out_2}
    }

# (3) Single-output many-to-one — any number of inputs ⇒ 1 unit, producing 1 output.
recipe link
    ingredients "build/*.o"
    cook "build/app" using {
        gcc {all} -o {out}
    }

# (4) Multi-output many-to-one — any number of inputs ⇒ 1 unit, producing 2 outputs.
recipe gen
    ingredients "src/*.rs"
    cook "out.js" "out.wasm" using {
        wasm-pack build
        cp pkg/main.js {out_1}
        cp pkg/main.wasm {out_2}
    }
```

In (1) and (3), `{out}` (no index) names the unit's single declared output. In (2) and (4), the unit declares two outputs and `{out}` is rejected; the indexed forms `{out_1}` and `{out_2}` name them in declaration order. In (1) and (2), `{in}` and `{in.ACCESSOR}` are valid (iteration is happening); in (3) and (4), they are rejected and `{all}` is the canonical input-side handle. `{out_N.ACCESSOR}` works identically in any mode that admits `{out_N}` — e.g., `mkdir -p {out_1.dir}` in (4) creates the parent directory of the first declared output.

The dep-driven case (§5.4) is the third one-to-one variant; it parallels (1) and (2) with `{lib.ACCESSOR}` driving the iteration in place of `{in.ACCESSOR}`. Inside the using-clause body, `{in}` still names each iteration item (now sourced from `lib`'s output list), so the body looks identical to the own-input form:

```cook
recipe install
    cook "/usr/lib/{libmath.name}" using {
        cp {in} {out}
    }
```

### 3.5. Lua block bindings (§6.4 unchanged in spirit, refined in scope)

The §6.4 bindings table is preserved. Iteration applies to Lua blocks the same way it applies to shell blocks: a Lua block in a one-to-one step runs once per iteration with `input` / `inputs` as the singleton current item, and `output` / `outputs` as the per-iteration computed output(s). A Lua block in a many-to-one step runs once with `inputs` as the full input list and `output` / `outputs` as the declared literal output(s).

This **fixes the many-to-one Lua wart** described in §1.4. The Lua block surface no longer conflates "single output → iterate" with "multi output → run once"; iteration is the output-pattern's call.

```cook
# Many-to-one Lua, finally clean:
recipe link
    ingredients "build/*.o"
    cook "build/app" using >{
        cook.sh("gcc " .. table.concat(inputs, " ") .. " -o " .. output)
    }
```

§B.6.1's rationale for keeping textual placeholders out of Lua blocks is preserved unchanged. Inside a Lua block, `{libmath}` is parsed as Lua syntax (a one-element table containing the local variable `libmath`), not as a placeholder. Authors who want cross-recipe references in Lua call `cook.dep_output("libmath")` or `cook.dep_output_list("libmath")`.

### 3.6. Cross-recipe substitution extends to shell blocks

§5.5's surface-list paragraph (today: "in a `using`-string, `plate` command, `test` command, or bare shell") is extended to:

> A `{NAME}` bare reference in a `cook` `using` shell block, `plate` command, `test` command, or bare shell MUST be substituted by the space-joined concatenation of the named recipe's output list (§{xref.dep-recipe-output}).

The `{libmath}` substitution thus works inside `using { … }` exactly as it works inside the (now-removed) `using "cmd"` form. The wart in §1.5 closes.

§5.4's firewall on `{lib.ACCESSOR}` in non-driving steps is preserved verbatim. `{lib.ACCESSOR}` is only valid in an output pattern (where it declares the driver); inside any using-clause body, `{lib.ACCESSOR}` is rejected — the author writes `{in.ACCESSOR}` if `lib` is the driver, and goes to Lua otherwise.

### 3.7. Substitution timing

Shell-block placeholder substitution happens at **register-time code generation**, mirroring the existing rule for `using "cmd"` (§B.6.1). By the time a unit is recorded with `cook.add_unit({command = "…"})`, the command field is concrete text; the cache key is observable per §8.6.

The reference implementation already substitutes `using "cmd"` at register time (`cook-luagen/src/cook_step.rs:96-103`, `template::expand_template_to_lua_with_deps`). This design extends the same substitution path to `UsingClause::ShellBlock` lines: each shell-block line is templated through `expand_template_to_lua_with_deps`, joined with `\n`, prepended with `set -e`, and recorded as the unit's `command`.

For a one-to-one shell block, codegen emits a `for` loop (the `OneToOne` arm at `cook_step.rs:71-126`) with the substituted block as the per-iteration command. For many-to-one, the BlockStep arm collapses to a single `add_unit` call with the substituted block. The mode-selection logic in `cook_step_mode` (`cook_step.rs:27-41`) is rewritten to read from the **output pattern** (§3.1's table), not from the using-clause's contents.

### 3.8. App. A grammar deltas

App. A.4 changes:

- Remove the `using_clause STR` production. `using_clause` and `block_using_clause` produce only `shell_block` or `using_lua_block`.
- Remove the "single-output: bare-string `using` shell" entry from the four-shape table in §4.5.
- §4.6's "multi-output rule" simplifies: the using-clause is *always* a block; there is no longer a "rejected" string variant to mention.

App. A.5 (placeholder grammar) gains a new production for the dotted accessor form admitted in using-clause bodies, `{NAME.ACCESSOR}` where `NAME ∈ { in, out, out_N }` — the recipe-name and accessor-split rules of §5.2 already produce this shape; the App. A entry is for completeness.

### 3.9. Tree-sitter parser deltas

`tree-sitter-cook`'s `grammar.js`:

- `using_clause`: drop the `field("command", $.string)` arm; keep only `field("shell", $.shell_block)` and `field("lua", $.using_lua_block)`.
- `block_using_clause`: unchanged (it already only carries the two block forms).
- Highlights query (`queries/highlights.scm`) drops the `(using_clause command: (string) @string.special)` rule.
- Injections query (`queries/injections.scm`) gains `(shell_block (shell_content) @injection.content (#set! injection.language "bash"))` — required by the unified shell-block surface (without it, the contents of `using {…}` highlight as plain text rather than as bash).

## 4. Migration

Pre-release lockstep posture. Every existing `using "cmd"` instance in the repo is rewritten to `using { cmd }`. Every existing bare `{stem}` / `{name}` / `{ext}` / `{dir}` in surface code is rewritten to `{in.stem}` etc. The rename is mechanical:

- `{stem}` → `{in.stem}`
- `{name}` → `{in.name}`
- `{ext}` → `{in.ext}`
- `{dir}` → `{in.dir}`

`{in}` and `{out}` keep their bare names (they're already the canonical names in the new model). `{all}` keeps its name.

Touched surfaces:

- `standard/src/content/docs/` — every `using "cmd"` example, every bare path-accessor.
- `standard/conformance/positive/` and `negative/` — fixtures that exercise `using "cmd"` get their canonical re-emission updated; new fixtures cover (a) literal-output many-to-one Lua block, (b) shell block with `{lib}` cross-recipe ref, (c) `{out_1}` / `{out_2}` indexed multi-output, (d) bare `{stem}` rejection diagnostic, (e) `{in}` in a literal-output step rejection diagnostic, (f) mixed-driver multi-output rejection diagnostic.
- `examples/*/Cookfile` — repo-internal example projects.
- `cook_modules/*.lua` — checked for any string-build paths that mention `{stem}` etc.; module Lua composes commands with the new vocabulary.
- `tree-sitter-cook/test/corpus/` — tree-sitter corpus fixtures parallel the conformance suite.

The reference implementation's `UsingClause::Shell(String)` AST variant is removed; the codegen drops its `OneToOne` and `ManyToOne` arms' `Some(UsingClause::Shell(cmd))` matches; mode selection moves to a single output-pattern analysis pass.

## 5. Implementation impact

### 5.1. `cli/crates/cook-lang`

- `ast::UsingClause` loses its `Shell(String)` variant.
- `cook_line.rs`'s using-clause parser drops the bare-string acceptance branch; only `>{` (Lua block) and `{` (shell block) dispatch.
- The diagnostic for the removed form names the migration target: "the `using \"cmd\"` form was removed in CS-NNNN; rewrite as `using { cmd }`."

### 5.2. `cli/crates/cook-luagen`

- `cook_step.rs::cook_step_mode` is rewritten to read iteration from `analyze_output_pattern` only. The `Some(UsingClause::Shell(cmd)) if cmd.contains("{in}")` branch is gone.
- A new `template::validate_placeholders(body, mode, declared_outputs)` pass walks each using-clause body and rejects placeholders that don't belong to the step's mode (§3.3 rules). Same pass handles bare `{stem}`-and-friends rejection.
- `template::expand_template_to_lua_with_deps` gains support for `{in.ACCESSOR}` / `{out.ACCESSOR}` / `{out_N}` / `{out_N.ACCESSOR}` token shapes.
- The `BlockStep` codegen arm gains placeholder substitution for shell-block lines (it currently emits them verbatim).
- `dep_ref::parse_dep_token` accommodates the new dotted forms; `{in.X}` and `{out.X}` are not dep tokens (no recipe name), so they fall through cleanly.

### 5.3. `tree-sitter-cook`

- Per §3.9. Conformance with the Standard's grammar changes; the parallel queries fix from the immediately-preceding work session lands as the first commit of the implementation series.

### 5.4. Conformance fixtures

The conformance harness at `standard/conformance/` regenerates against the new AST. Every fixture using `using "cmd"` updates its surface and its expected canonical re-emission. The new fixtures listed in §4 cover the new diagnostics.

## 6. Open questions

None blocking. Two design choices the spec makes explicitly that the implementation plan should call out as decisions:

1. **`{out.ACCESSOR}` admitted.** §3.3 admits path-accessor composition on `{out}` and `{out_N}`. An alternative is to admit only `{out}` and `{out_N}` and require Lua for `path.dir(out)`. The spec admits the dotted form because (a) it matches the `{NAME.ACCESSOR}` shape used everywhere else in the model, and (b) the `mkdir -p {out.dir}` use case is common enough that forcing Lua for it would push authors into the wrong surface.

2. **No `{in_N}` indexed input form.** Multi-input cases use `{all}` (space-joined) only; an author who needs the third input by index goes to Lua. The asymmetry with `{out_N}` is deliberate: outputs are *declared* by the cook step (their count and order are known statically), inputs come from `ingredients` resolution (their count is dynamic). Indexed input access at the placeholder layer would be either unbounded (a `{in_42}` query whose existence depends on glob results) or arbitrarily capped. Keeping the surface tight is the better trade.

## 7. Rationale (informative annex draft for App. B.6.x)

A new App. B.6.x rationale subsection covers four points to be added to the Standard alongside the normative changes:

- **Why output-pattern is the iteration source.** The output pattern is *declarative* — it says what files this step produces and (via `{in.X}` or `{lib.X}`) what shape they take. Iteration falls out: if the filenames are parameterized, the step iterates; if they're literal, it doesn't. Reading iteration from the using-clause's contents conflated "what to run" with "how often to run it" and produced silent footguns when authors put `{in}` in a literal-output step.
- **Why `using "cmd"` is removed.** With shell blocks first-class and single-line blocks legible (`using { cmd }`), the string form bought one character of brevity at the cost of a separate normative branch in §6.7. The cost is paid every time an author moves a one-line command onto two lines.
- **Why bare path-accessors are removed.** Two spellings of the same accessor (`{stem}` vs `{in.stem}`) with different validity contexts (output: bare-only; using-clause: dotted-only under previous proposals) is the kind of position-dependent rule that a Standard pays for forever. One spelling, valid in both positions, is worth the v0.4 churn.
- **Why `{lib.ACCESSOR}` stays rejected in using-clauses.** The §5.4 firewall is what makes "iteration is owned by the output pattern" enforceable. Allowing `{lib.ACCESSOR}` in the body would let authors smuggle a second iteration source past the output-pattern declaration, undoing §3.1's single-driver invariant.

## 8. Acceptance criteria

The Standard PR for CS-NNNN is acceptance-complete when:

1. §4.5, §4.6, §5.4, §5.5, §6.7, App. A.4, App. A.5, and App. B.6 are updated as described.
2. The conformance fixtures listed in §4 exist and pass.
3. The reference implementation (`cook-lang`, `cook-luagen`) and `tree-sitter-cook` parse, codegen, and highlight the new surface; the `cargo test --workspace` and `cargo test -p cook-lang --test conformance` suites are green.
4. Every Cookfile in `examples/`, `cook_modules/`, the top-level repo, and the `tree-sitter-cook/` subproject uses the new surface.
5. The standard's "Recent Changes" appendix (App. D) gets an entry naming this CS-NNNN and the migration recipe.
