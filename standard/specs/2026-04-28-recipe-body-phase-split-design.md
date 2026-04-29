# Design: Recipe-body phase split (`>` execute / `>>` register / module_call register)

**Date:** 2026-04-28
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Scope:** Cook Standard, the Rust parser (`cli/crates/cook-lang`), the codegen (`cli/crates/cook-luagen`), and `tree-sitter-cook`. The Standard change is the source of truth; the implementation work follows lockstep per `CONTRIBUTING.md`.

## 1. Motivation

The current Standard places every Lua surface form in one bucket: code in `> expr`, in `>{ … }`, and in module-call lines (`m.fn(…)` per §4.11) all evaluates during the **register phase**. The execute-phase Lua surface is the `using >{ … }` clause on a `cook` step, and only that clause.

This is a real conceptual cost:

1. **Pedagogy.** A user encountering `> print("hi")` reasonably expects "this prints when I run the recipe." Today it prints during planning, before any unit runs. The first `>` someone types violates their mental model.
2. **No clean answer to "I want a runtime hook."** The user is forced to either (a) wrap the body in a `cook` step with `using >{ … }` (overkill for a side effect with no inputs/outputs), or (b) accept that their `> print(...)` runs at a different time than they expected.
3. **Asymmetry with shell.** A bare `echo X` line produces an execute-phase unit — runs when the recipe runs. A bare `> print("X")` line evaluates at register. Two surface forms, both look like "run this," with opposite timing. Nothing in the surface signals which is which.
4. **Module-call status is unclear in the spec.** §4.11 desugars `m.fn(…)` to lua_line / lua_block; §8.3's table classifies those as register-phase; the chain works, but it's spread across three sections and nothing says "module calls are register-phase orchestration" outright.
5. **Body-unit shape is undefined.** When `> print("X")` is the only body content, what unit does it produce? Today: zero. Under any "execute-phase" model, the answer needs to be specified — inputs, outputs, cache, scheduling, bindings, API surface.

This design splits the Lua surface into two clearly-named forms, locks down what executes when, defines the unit shape for execute-phase Lua, and adds a region-ordering rule that makes the two-phase model textually visible in every recipe.

## 2. Non-goals

- **Bundling beyond same-region/same-bundle adjacency.** The rule below coalesces consecutive imperative-region steps into one execute-phase unit; it does NOT merge imperative-region code across cook/plate/test/`@` boundaries, and it does not introduce any user-controlled bundling syntax beyond `>{ … }`.
- **Cross-kind state sharing.** A coalesced shell call inside a body unit does not share `cwd` with a separate coalesced shell call across a `@` boundary — each is a fresh process. The Lua VM, however, persists across the whole body unit (this is the asymmetry the design accepts).
- **Persistent shell sessions across the whole recipe.** Holding a shell open across cook step groups, `@` units, etc., is meaningfully more complex than `set -e` + newline-joined script. Out of scope.
- **A new step kind for "interactive Lua".** `@` stays shell-interactive; there is no `@>` form.
- **Changes to `using >{ … }` semantics.** The using-clause Lua already runs at execute with §6.4 bindings; this design preserves that and slots it into the phase classification table.
- **Migration tooling.** Pre-1.0 lockstep posture; existing example Cookfiles update in the same change set.

## 3. Design

### 3.1. Phase classification (normative table)

A new normative subsection in §8 records every place a Lua expression can appear and the phase that evaluates it. The table is the single landing place a reader hits when asking "when does this run."

| Surface form | Source | Phase | Produces a unit? |
|---|---|---|---|
| `config` block body | §3.6, §3.6.1 | **Load** (before register) | No — runs against Cook Lua API state directly. |
| Cook module top-level + `init()` (§7.5) | §7 | **Load** (when `use` resolves) | No. |
| `>>` line | §4.9 (new) | **Register** | No — inlined into the recipe-body fn. |
| `>>{ … }` block | §4.9 (new) | **Register** | No — inlined. |
| `module_call` (`m.fn(…)`) | §4.11 | **Register** | No — desugared to inline_lua_line / inline_lua_block. |
| `>` line | §4.9 | **Execute** | Yes (see §3.4 below for the body-bundling rule). |
| `>{ … }` block | §4.9 | **Execute** | Yes. |
| `cook STR using >{ … }` | §4.7 | **Execute** | Records one unit with `lua_code` payload (§6.4 bindings apply). |
| `cook.add_unit({lua_code = "…"})` | §6.3 | **Execute** | The unit's payload runs at execute. |

Cross-link: each row's "Source" column resolves to the section that grammatically defines the form.

### 3.2. Surface forms: the `>` / `>>` split

`>` and `>{ … }` are redefined as **execute-phase Lua**. `>>` and `>>{ … }` are introduced as **register-phase (inline) Lua**. Module-call lines retain their current semantics (register-phase, desugared to inline_lua_line / inline_lua_block).

| Prefix | Name | Phase | Body content |
|---|---|---|---|
| `>` *(text)* `\n` | `lua_line` | execute | one Lua statement (or expression statement) |
| `>{` *(text up to balanced `}`)* | `lua_block` | execute | one Lua chunk |
| `>>` *(text)* `\n` | `inline_lua_line` | register | one Lua statement |
| `>>{` *(text up to balanced `}`)* | `inline_lua_block` | register | one Lua chunk |

Lexical rules:

- `>>{` and `>>` are lexed before `>{` and `>` (longest-prefix wins).
- `>>>` and longer runs are reserved (currently rejected).
- A `>>` line may be empty (the `>>` token alone, no body), matching the existing rule that `>` may be empty.
- The brace-balance algorithm of §2.9 is shared between `>{` / `}` and `>>{` / `}`.

### 3.3. Recipe-body region rule

A `recipe_body` is grammatically split into two regions, in order. Each region MAY be empty. Once any imperative-region step appears, no declarative-region step is permitted afterward in the same recipe.

| Region | Steps | Phase |
|---|---|---|
| Declarative | `ingredients`, `cook`, `plate`, `test`, `inline_lua_line`, `inline_lua_block`, `module_call`, comments, blank lines | register |
| Imperative | `lua_line`, `lua_block`, `shell_command`, `interactive_command`, comments, blank lines | execute |

Ordering rule (normative): a conforming implementation MUST reject a recipe in which a declarative-region step appears after the first imperative-region step of the recipe body. The diagnostic MUST identify the offending step's line and the line on which the imperative region began.

Comments and blank lines do NOT trigger the region transition. A `comment` between an imperative step and a declarative step is still illegal — the transition is governed by the most recent step, not the most recent line.

#### 3.3.1. Examples

Valid:

```cook
recipe foo
    >> local v = cook.env.VARIANT or "debug"     -- declarative (register)
    cook "out.{v}" using "build {v}"             -- declarative (register)
    > print("build complete")                    -- imperative (execute)
    > os.exit(0)                                 -- imperative (execute)
end
```

Valid (imperative-only):

```cook
recipe smoke
    > print("hello")
    echo "world"
end
```

Valid (declarative-only):

```cook
recipe build
    cook "out" using "make"
end
```

Invalid:

```cook
recipe foo
    cook "a" using "..."
    > print("midway")
    cook "b" using "..."   -- ERROR: declarative step after imperative region began on previous line
end
```

Diagnostic shape: `recipe foo: cook step on line 4 is not allowed after the imperative region began on line 3 (\`>\` step). Move declarative steps (cook/plate/test/ingredients/>>/module-call) above the first \`>\` / \`>{ … }\` / shell / \`@\` step, or split into two recipes.`

### 3.4. Body-unit bundling

The imperative region of a recipe compiles to a sequence of execute-phase units, bounded by `@interactive` steps:

- A run of consecutive imperative steps that contains zero `@` steps coalesces into **one body unit** (a `cook.add_unit({lua_code = …, cache = false})` call; no inputs, no outputs).
- An `@interactive` step is its own draining unit (existing §8.5 semantics) and breaks the bundle.

Inside one body unit, the worker VM evaluates the bundle's Lua chunk. Source-order is preserved:

- A `lua_line` (`> expr`) becomes one Lua statement in the chunk.
- A `lua_block` (`>{ … }`) becomes a Lua block (or a `do … end` wrapping; see Open Question §6.1) in the chunk.
- A `shell_command` line becomes a `cook.sh("…")` call.
- **Adjacent `shell_command` lines coalesce** into a single `cook.sh` call whose argument is the lines joined by `\n` and prefixed with `set -e`. A `lua_line` or `lua_block` between two shell lines breaks the coalescence — the second shell call is a fresh process.

The Lua VM persists across the whole body unit, so a local declared in one `>` line is visible in the next `>` line of the same bundle. (This is the "Make-`.ONESHELL`-for-Lua" rule.)

#### 3.4.1. Worked example

```cook
recipe foo
    cook "out" using "..."        -- declarative
    cd bin                        -- imperative bundle 1 starts
    ./app
    > local x = "ok"
    echo "$x done"
    @./repl
    > print("after repl")         -- imperative bundle 2 starts
    echo "tail"
end
```

Compiles to the recipe-body Lua function:

```lua
cook.recipe("foo", {…}, function()
    -- declarative region
    cook.step_group(function()
        cook.add_unit({inputs = {…}, output = "out", command = "..."})
    end)

    -- imperative bundle 1 (one execute-phase Lua unit)
    cook.add_unit({
        lua_code = [[
            cook.sh([[set -e
cd bin
./app]])
            local x = "ok"
            cook.sh("set -e\necho \"$x done\"")
        ]],
        cache = false,
    })

    -- @interactive (own draining unit; existing semantics)
    cook.add_unit({command = "./repl", interactive = true, cache = false})

    -- imperative bundle 2 (fresh Lua VM; x not visible here)
    cook.add_unit({
        lua_code = [[
            print("after repl")
            cook.sh("set -e\necho \"tail\"")
        ]],
        cache = false,
    })
end)
```

Three execute-phase units in the imperative region (two body units + one interactive), one step group from the cook step. The body units run sequentially after the cook step group's barrier; `cd` persists from line 1 to line 2 of bundle 1 (one `cook.sh` call); `x` persists from `> local x = "ok"` to `echo "$x done"` (same Lua VM); but neither `cd` nor `x` cross the `@./repl` boundary.

### 3.5. API surface inside an execute-phase Lua unit

The body unit's Lua VM is execute-phase. Per §3.1's classification, register-only helpers are unavailable. The following surface IS available inside an execute-phase Lua unit:

- Standard Lua library (`print`, `string`, `table`, `math`, `io`, `os`, `pcall`, etc.).
- `cook.env` — read-only access to the resolved environment (writes inside an execute-phase unit MUST raise a Lua error; semantics ratified at register, frozen at execute).
- `cook.cache` — read and write.
- `cook.platform` — read.
- `fs.*` — all current functions (read/write/glob/exists/mkdir_p, etc.).
- `path.*` — all current functions.
- `cook.sh(cmd)` — **promoted to both phases**. In execute, it spawns `/bin/sh -c cmd` with the unit's environment, captures stdout (returned as a string), streams stderr to the worker's stderr, and raises a Lua error on non-zero exit (message prefix `COOK_CMD_FAILED:`, matching register-phase contract). The "execute-phase cook.sh" is the mechanism through which coalesced shell lines run.

The following surface is NOT available inside an execute-phase Lua unit:

- `cook.add_unit` — registration is closed; raise a Lua error if called.
- `cook.exec`, `cook.interactive` — register-phase recorders; raise a Lua error if called.
- `cook.step_group` — register-phase; raise a Lua error if called.
- `cook.recipe` — register-phase; raise a Lua error if called.

§6.3.1 amendment: the existing register-phase semantics of `cook.sh` are unchanged. The amendment adds the execute-phase row to the phase column ("both phases"), specifies the execute-phase semantics above, and notes that the function name is intentionally shared because the user-visible behavior — "run this command and give me its stdout, error on non-zero" — is the same; only the surrounding scheduling differs.

§6.3.2 amendment: `cook.exec` and `cook.interactive` remain "register-phase only." A note clarifies that calling them inside an execute-phase Lua unit (`>`, `>{ }`, or a `using >{ … }` payload) is a Lua runtime error.

### 3.6. Using-clause Lua: clarification, not change

The `cook STR using >{ … }` clause already records a unit with a `lua_code` payload (§6.4) that runs at execute with `input` / `output` / `inputs` / `outputs` / `input_N` bindings. This design adds nothing to that semantics. The classification table in §3.1 captures it as a separate row: same surface (`>{ … }`), additional bindings provided by the surrounding `using` clause.

A `using >>{ … }` form is rejected at parse time. The using clause produces work, not orchestration; "register-phase Lua produces a using clause" is incoherent.

A `using >` (single-line `>`) form is also accepted for symmetry with `using >{ … }`: the entire `using` clause is one execute-phase Lua expression. Whether this is worth specifying or whether `using` should require a block is an Open Question (§6.4).

### 3.7. Module-call (§4.11): clarification, not change

Module-call lines remain register-phase. §4.11's existing desugaring rule (single-line → lua_line; multi-line → lua_block) is updated to: **single-line → `inline_lua_line`; multi-line → `inline_lua_block`** (the `>>` family). This preserves the register-phase semantics that module authors depend on (calling `cook.add_unit`, `cook.export`, etc., during recipe registration). No change to user-visible module-call surface.

### 3.8. Spec sections affected

| Section | Change |
|---|---|
| §2.7 (line prefixes) | Add `>>` and `>>{` to the line-prefix table. Update the cascade. |
| §3.7 (recipe body grammar) | Replace `recipe_item*` with `declarative_region imperative_region?`. |
| §3.8 (step dispatch) | Split the cascade into "declarative-region cascade" and "imperative-region cascade." Add the region-transition rule. |
| §4.9 (lua-steps) | Rewrite. Split `lua_line` / `lua_block` (execute) from `inline_lua_line` / `inline_lua_block` (register). Specify the body-unit bundling rule. |
| §4.11 (module-call) | Update desugaring target from lua_line/lua_block to inline_lua_line/inline_lua_block. |
| §6.3.1 (cook.sh) | Promote to both phases. |
| §6.3.2 (cook.exec, cook.interactive) | Add note: calling these from an execute-phase Lua unit is a runtime error. |
| §6.4 (using-block globals) | Cross-link to the §3.1 phase table; the `using >{ … }` row maps here. |
| §8.2 (two-phase execution) | Add the §3.1 phase classification table. |
| §8.3 (step groups) | Update the surface-construct table: lua_line/lua_block become "execute-phase, sequential body unit (coalesced)." Add inline_lua_line / inline_lua_block as new rows ("register-phase, no unit"). |
| App. A.3 (recipe grammar) | New `recipe_body` production with `declarative_region` and `imperative_region`. |
| App. A.4 (steps) | Add `inline_lua_line` and `inline_lua_block` productions. Update step-dispatch priority lists per region. |
| App. A.5 (primitives) | Add the `>>` and `>>{` lexical rules. |
| App. B.4 (rationale) | New subsections: (a) why split `>` and `>>`, (b) why module-call defaults to register, (c) why region ordering, (d) why body bundling per recipe rather than per line. |

## 4. Examples (informative)

### 4.1. A recipe that uses both regions

```cook
use cpp

recipe build
    >> local std = cook.env.CXX_STD or "c++20"
    ingredients "src/*.cpp"
    cook "build/{stem}.o" using >{
        cook.sh(string.format("%s -std=%s -c %s -o %s",
            cook.env.CXX or "g++", std, input, output))
    }
    cook "build/app" using >{
        cook.sh(string.format("%s %s -o %s",
            cook.env.CXX or "g++", table.concat(inputs, " "), output))
    }
    > print("build complete; binary at build/app")
end
```

The `>>` line and both `cook` steps are declarative; `std` is a register-phase local feeding the `using` clauses' string formatting. The `>` line is imperative and runs after both cook units finish.

### 4.2. A run-only recipe

```cook
recipe smoke
    cd build
    ./app --selftest
    > if not fs.exists("build/.smoke.ok") then error("smoke failed") end
end
```

No declarative region. The whole body is one execute-phase Lua unit. Two shell lines coalesce into one `cook.sh` call (so `cd` persists). The `>` line runs in the same VM after the shell call completes.

### 4.3. A recipe rejected by the region rule

```cook
recipe bad
    cook "a" using "..."
    > log("midway")
    cook "b" using "..."
end
```

Rejected: the `cook "b"` step on line 4 follows the `>` step that started the imperative region on line 3.

### 4.4. Module-call orchestration

```cook
use cpp

recipe lib
    cpp.static_library("widget", { sources = { "src/widget.cpp" } })
    > print("built libwidget.a")
end
```

`cpp.static_library` runs at register; it records cook units. The `>` line runs at execute, after those units complete.

## 5. Conformance fixtures (added or updated)

### 5.1. New positive cases

- **`>` is execute-phase**: a recipe with a `>` line that calls `cook.sh("echo hi")`; the `parse.txt` shows one execute-phase Lua unit recorded, no register-phase Lua-only step.
- **`>>` is register-phase**: a recipe with `>> cook.add_unit({command = "echo hi", cache = false})`; the unit is recorded by the `>>` line at register, runs at execute. The dump shows the unit, not an inline_lua_line step.
- **Body bundling: two `>` lines + one shell**: produces one execute-phase Lua unit whose payload joins all three statements (with shell coalesced into one `cook.sh` call).
- **Body bundling: `@` breaks the bundle**: a recipe with `> a; @cmd; > b` produces three units (body, interactive, body).
- **Module call still register-phase**: the existing 010 case (`cpp.bin(...)`) updates its `parse.txt` to show inline_lua_line classification, otherwise unchanged behavior.
- **Using-clause Lua unchanged**: the existing 007 case (multi-output cook with lua block) unchanged.

### 5.2. New negative cases

- **Imperative-then-declarative ordering**: `cook "a" using "..."; > log("x"); cook "b" using "..."` — error class `imperative-then-declarative`.
- **`using >>{ }` rejected**: `cook "x" using >>{ ... }` — error class `using-register-block`.
- **`cook.add_unit` from execute phase**: `> cook.add_unit({...})` — runtime error class `register-only-from-execute` (semantic; the Rust harness asserts the Lua error message contains the diagnostic substring).

## 6. Open questions

### 6.1. `do … end` wrapping for `>{ }` blocks inside a body unit

Should each `>{ … }` block in a coalesced body unit be wrapped in `do … end`? Pro: scopes `local` declarations to the block, matching the user's `>{ }`-as-block intuition. Con: subtly different from the "bundle-shares-VM-scope" rule. Decision pending; default is "no wrapping" (bundle-shared scope) unless someone surfaces a use case for block-scoped locals inside a bundle.

### 6.2. `>>` block in declarative region: phase ambiguity?

A `>>{ … }` block can call `cook.add_unit` or `cook.export` to record work. That's by design — it's register-phase orchestration. But it could also call `cook.sh`, which has different semantics in register (synchronous, captured) vs. execute. The §3.1 table says inline_lua_block is register-phase, so `cook.sh` inside it is register-phase. Worth a normative note.

### 6.3. Empty `>` and `>>` lines

`>` empty (just `>` followed by newline) is valid today and produces an empty `lua_line`. Under this design, an empty `>` would compile to no statement in the body unit's payload. An empty `>>` would compile to no register-phase code. Both are valid no-ops. Confirm or reject? Default: keep as no-ops, document as such.

### 6.4. `using >` (single-line) acceptance

Today the using-clause accepts `using >{ block }` and `using "string"` and `using { shell-block }`. There is no `using > expr` form (single-line lua). Is it worth adding for parity with the `>` / `>{ }` split outside `using`? Default: no — keep using requiring a block for Lua, consistent with the existing rule that the using clause introduces a payload, not a statement.

### 6.5. Position of `ingredients` within the declarative region

`ingredients` is at-most-one (App. A.3). Is it required to be the FIRST step of the declarative region, or can it appear after some `>>` / `cook` / `plate`? Today it can appear anywhere among the declarative steps; this design keeps that. Worth confirming in the new §3.7 prose.

### 6.6. Diagnostic surface for the region rule

The proposed diagnostic ("declarative step after imperative region began on line N") fires at parse time. An alternative is to fire it at register-time validation, with line numbers from the AST. Parse-time is sharper (no register evaluation needed); register-time is more flexible (could include cross-step context). Default: parse-time.

### 6.7. Cross-recipe visibility of the body unit

Cook's cross-recipe references (§{xref}) target a recipe's outputs, ingredients, and accessors. A body unit produces no outputs (no `output` field on the recorded unit). Cross-recipe dependents should NOT observe a body unit at all — only the cook/plate/test outputs of the declarative region. This is implicit in the unit shape but worth a normative sentence.

## 7. Implementation impact

- **Lexer (`cli/crates/cook-lang/src/lexer.rs`)**: add `>>` and `>>{` token classes. Adjust the prefix-resolution order so `>>{` and `>>` are lexed before `>{` and `>`.
- **Parser (`cli/crates/cook-lang/src/recipe.rs`)**: produce `Step::InlineLuaLine` and `Step::InlineLuaBlock` AST variants. Add the region-tracking flag during recipe-body parsing and the diagnostic for region transitions. Update §4.11 desugaring (single-line module-call → `Step::InlineLuaLine`; multi-line → `Step::InlineLuaBlock`).
- **Codegen (`cli/crates/cook-luagen/src/recipe.rs`)**: emit `Step::InlineLuaLine` / `Step::InlineLuaBlock` as inlined Lua (current `Step::Lua` / `Step::LuaBlock` behavior). Emit `Step::Lua` / `Step::LuaBlock` (execute-phase) into the bundling pass:
  - Walk the imperative region in order.
  - Coalesce consecutive `Step::Shell` into one `cook.sh` call (newline-joined, `set -e` prefix).
  - Coalesce consecutive `Step::Lua` / `Step::LuaBlock` and the cook.sh-bundled shell into one `cook.add_unit({lua_code = …, cache = false})` call.
  - Each `Step::Interactive` ends a bundle and emits its own `cook.add_unit({command = …, interactive = true, cache = false})` call.
- **Cook Lua API (`cli/crates/cook-register`)**: promote `cook.sh` to also exist on the execute-phase worker VM. Restrict `cook.add_unit` / `cook.exec` / `cook.interactive` / `cook.step_group` / `cook.recipe` to register-phase only with a clear runtime error.
- **Tree-sitter (`tree-sitter-cook/grammar.js`, `src/scanner.c`)**: add `>>` and `>>{` line prefixes; add `inline_lua_line` and `inline_lua_block` rules; restructure `_recipe_item` to produce two regions; add a parse-time error for region-transition violations (or accept syntactically and let a follow-up validation pass diagnose, mirroring the Rust parser).
- **Conformance corpus (`standard/conformance/`)**: update existing fixtures (`008-lua-line-and-block`, `010-use-and-module-call`); add new fixtures per §5.

## 8. Migration

Pre-1.0 lockstep. The implementation change ships in the same commit as the spec change. Existing examples in `examples/` and `cli/crates/cook-lang/src/tests.rs` update to the new surface in the same change set. The conformance corpus updates per §5.

External Cookfiles (none currently; `project_git_hosting.md` notes there are no external users) using `> local x = …` for register-time orchestration migrate to `>> local x = …`. A grep for `^[[:space:]]*>[^>{]` is the migration scope.

## 9. Risks and alternatives

### 9.1. `>>` is ugly

The double-arrow is unusual. Alternatives considered: `>!`, `:`, `&`, `:>`, `^>`. None are clearly better. `>>` has the virtue of "more arrow = more cook-y" and works lexically (the `}`-balance prefix `>{` and `>>{` are both unambiguous).

### 9.2. The region rule is a syntactic constraint that could be a semantic one

We could parse mixed regions and emit a semantic warning rather than a parse error. Argument for parse-time: the rule is structural, and a parse error is the sharpest possible diagnostic. Argument for semantic: more flexible if someone wants to relax the rule later. Default is parse-time per Open Question §6.6; the spec and grammar express the rule structurally.

### 9.3. Body bundling is invisible to the user

A recipe with five `>` lines and three shell lines compiles to one body unit whose progress UI shows one entry instead of eight. That's a step backwards from today's per-shell-line UI granularity. Mitigation: the unit's progress entry includes the bundle's source-line range; per-line output still streams (with prefixes) as the bundle runs. Worth a UI note in the implementation plan, not the spec.

### 9.4. The "fresh shell after `>`" gap

Inside one body unit, `cd bin` then `> log("x")` then `pwd` gives a `pwd` from the original directory, not `bin`, because the second `cook.sh` call is a fresh shell process. The user diagrams it; the spec documents it; the alternative (long-lived shell process held by the Lua VM, fed lines via stdin) is significantly more complex. Accept the gap.

### 9.5. The `>` / `>>` split adds a syntactic form

That's the cost. The benefit is the pedagogical and semantic clarity laid out in §1. Not splitting leaves the "what runs when" question unresolved at the surface level — the alternative we considered and rejected.

## 10. Decision deadline

This design ships as part of v0.3 or v0.4 (whichever cut takes the next batch of CSes). It is not a v0.2 patch — too large for a clarification. Once accepted, it sequences:

1. **CS-NNNN-A** (this design landed): the §3.1 phase classification table against current behavior. Mechanical clarification, no behavior change. Ships first to lock the "current state" snapshot.
2. **CS-NNNN-B**: the surface split (`>` execute / `>>` register / module_call register) and the region rule. Behavior change. Ships when the design lands.
3. **CS-NNNN-C** (implementation): Rust parser, codegen, tree-sitter, conformance fixtures. Ships in lockstep with CS-NNNN-B per §{intro.conformance}.

Reviewers: please mark up §6 (open questions) with answers before promoting to the implementation plan.
