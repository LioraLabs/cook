# Cook Standard — Skeleton PR Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the Cook Standard as the authoritative specification of the Cookfile language by landing a skeleton document tree under `docs/standard/`, a complete formal grammar (Appendix A), a conformance corpus + `cook-lang` test harness, and the surrounding discipline artifacts (CONTRIBUTING.md, pre-commit hook, normative-keyword lint, architecture-doc banners).

**Architecture:** Markdown (`.mdx`) documents under `docs/standard/`, split into normative chapters (`00`–`07`, `A`) and informative files (`B`, `C`, `D`). Chapters 00, 01, and Appendix A are fully written; chapters 02–07 ship as skeletons with numbered section outlines and `NORMATIVE-TODO` stubs. The conformance corpus (positive and negative Cookfiles) is consumed by a Rust integration test in `cli/crates/cook-lang/tests/conformance.rs`. A portable `.githooks/pre-commit` script enforces the spec-first rule locally. No `.github/` artifacts (the repo is hosted on Soft Serve).

**Tech Stack:** Markdown/MDX (content); Rust (test harness, using existing `cook-lang` crate); Bash (pre-commit hook, normative-keyword lint); EBNF (W3C-style, used in Appendix A).

**Source-of-truth rule (applies throughout):** The authoritative reference for current Cookfile behavior is the Rust parser in `cli/crates/cook-lang/` — specifically `src/lexer.rs`, `src/recipe.rs`, `src/cook_line.rs`, `src/lua_block.rs`, `src/shell_block.rs`, and `src/ast.rs`. `tree-sitter-cook/grammar.js` is known stale and MUST NOT be consulted for authoritative behavior in this plan. Bringing tree-sitter into conformance is the subject of a follow-up plan (`CS-0002`).

**Conventions used in the Standard (the plan makes these concrete):**

- RFC 2119 keywords in all-caps for normative weight.
- Section numbering `§ N`, `§ N.M`, `§ N.M.K` (max depth 4); appendices `App. X`.
- W3C-style EBNF: `::=` definition, `|` alternation, `*` `+` `?` repetition, `( )` grouping, `"literal"` terminals, `/regex/` character classes, `lower_snake_case` productions, `UPPER_SNAKE_CASE` terminal classes.
- Every chapter begins with a one-line normativity banner.
- Stable `CS-NNNN` IDs for change entries; `CS-stub-NN` markers for skeleton stubs.

---

## Task 1 — Create the `docs/standard/` directory

**Files:**
- Create: `docs/standard/` (directory — ensured by creating the placeholder README)
- Create: `docs/standard/.gitkeep` (to ensure the directory is tracked even if later steps move files)

- [ ] **Step 1: Create the directory with a placeholder file**

```bash
mkdir -p docs/standard/conformance/positive docs/standard/conformance/negative
touch docs/standard/.gitkeep
```

- [ ] **Step 2: Verify the tree**

```bash
find docs/standard -maxdepth 2 -print
```

Expected output (exact order may vary):
```
docs/standard
docs/standard/.gitkeep
docs/standard/conformance
docs/standard/conformance/negative
docs/standard/conformance/positive
```

- [ ] **Step 3: Commit**

```bash
git add docs/standard/.gitkeep
git commit -m "docs(standard): create docs/standard/ directory for the Cook Standard"
```

---

## Task 2 — Write `00-introduction.mdx`

**Files:**
- Create: `docs/standard/00-introduction.mdx`

- [ ] **Step 1: Write the chapter**

Create `docs/standard/00-introduction.mdx` with the exact content below:

```mdx
# 0. Introduction

> **Normative.** This chapter defines the scope of the Cook Standard and the conventions the Standard uses.

## 0.1. Purpose

The Cook Standard is the authoritative specification of the Cookfile language. It defines the lexical structure, syntactic grammar, execution semantics, Cook Lua API, and module system. A Cookfile that conforms to the Standard is a _conforming Cookfile_; a tool that parses and executes Cookfiles in accordance with the Standard is a _conforming implementation_.

## 0.2. Scope

The Standard covers the full language surface a Cookfile author may use:

- Lexical structure (tokens, lines, prefixes). See § 2.
- Syntactic grammar (productions, ordering). See § 3 and App. A.
- Recipe structure and step kinds. See § 4.
- Execution semantics (register phase, execute phase, step groups, ingredient-serves matching, cache semantics). See § 5.
- The Cook Lua API exposed in `>` / `>{ ... }` / `using >{ ... }` contexts. See § 6.
- The module system (`use`, `import`, `cook_modules/` resolution, authoring contract). See § 7.

## 0.3. Non-scope

The Standard does NOT cover:

- The `cook` CLI surface (subcommands, flags).
- Exit codes, error message wording, or diagnostic formatting.
- Performance characteristics, including scheduler thread counts.
- The on-disk cache layout or the hash function used.
- Terminal output formatting.

These are implementation concerns and are documented in `docs/architecture/`.

## 0.4. Normative and informative material

Chapters `00` through `07` and Appendix A are normative. Appendices B, C, and D are informative. Within a normative chapter, **Example** and **Note** blocks are informative unless explicitly marked otherwise; the surrounding paragraphs are normative when they contain RFC 2119 keywords. See § 1.3 for the full convention.

## 0.5. Version stance

The Standard tracks the state of `main`. There is no version pragma in the Cookfile header at this time. A future Cook 1.0 release may introduce dual-track versioning (tagged snapshots alongside head-of-main); this is out of scope for the present draft.

## 0.6. Relationship to `docs/architecture/`

`docs/standard/` defines what Cookfiles **mean**. `docs/architecture/` documents how the current reference implementation **works** (parser module layout, runtime internals, scheduler thread-pool design, cache layout). Implementation documents may evolve freely without touching the Standard, provided they do not make normative claims about language behavior.

## 0.7. Conformance

A conforming implementation:

1. MUST accept every Cookfile in the Standard's positive conformance corpus (`docs/standard/conformance/positive/`).
2. MUST reject every Cookfile in the Standard's negative conformance corpus (`docs/standard/conformance/negative/`) with a diagnostic that identifies the offending line. The exact wording is implementation-defined; the diagnostic class is normative.
3. For accepted Cookfiles, MUST produce a parse whose structural shape matches the expected shape recorded for each case. Where a chapter of the Standard is presently stubbed (`NORMATIVE-TODO`), the corpus is the operative authority for that construct until the prose is written.

See § 7 of the Standard (Modules) for additional module-specific conformance requirements.
```

- [ ] **Step 2: Commit**

```bash
git add docs/standard/00-introduction.mdx
git commit -m "docs(standard): add § 0 Introduction"
```

---

## Task 3 — Write `01-notation.mdx`

**Files:**
- Create: `docs/standard/01-notation.mdx`

- [ ] **Step 1: Write the chapter**

Create `docs/standard/01-notation.mdx` with the exact content below:

```mdx
# 1. Notation and conventions

> **Normative.** This chapter defines the typographic, grammatical, and structural conventions used throughout the Standard. Conforming implementations and Standard authors rely on these conventions for correct interpretation.

## 1.1. Normative keywords

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED, NOT RECOMMENDED, MAY, and OPTIONAL, when appearing in all capitals in this Standard, are to be interpreted as described in RFC 2119 and RFC 8174.

A paragraph that does NOT contain one of these all-caps keywords is descriptive, not normative. Lowercase occurrences of "must", "shall", "should", or "may" are not normative — such wording MUST be either promoted to all-caps (making the clause normative) or rephrased (keeping the clause descriptive).

## 1.2. Section numbering and citation

Chapters are cited as `§ N`. Sections within a chapter are cited as `§ N.M`, subsections as `§ N.M.K`, and deepest-level subsections as `§ N.M.K.P`. Nesting deeper than four levels MUST NOT appear in this Standard.

Appendices are cited as `App. X` (where X is a letter). Appendix subsections use the same dotted form: `App. A.1`, `App. A.2.3`.

Examples within a section are numbered locally: `Example N.M.1`, `Example N.M.2`, and so on. Numbering restarts at 1 within each section.

## 1.3. Normative and informative blocks

Each chapter and appendix begins with a single-line banner:

- `> **Normative.** …` — the chapter establishes binding requirements.
- `> **Informative.** …` — the chapter is non-binding commentary.

Within a normative chapter:

- **Example** blocks are informative. They illustrate, but do not define, language behavior.
- **Note** blocks are informative unless explicitly marked otherwise.
- **Rationale** blocks are informative.
- All other prose is normative when it contains an RFC 2119 keyword.

## 1.4. Grammar notation

The Standard uses a W3C-style EBNF variant. The full formal grammar is in Appendix A. The productions in § 3 and § 4 are prose walkthroughs of the productions in Appendix A; where the two disagree, Appendix A is authoritative and the prose is a bug.

The notation is:

| Symbol | Meaning |
|---|---|
| `::=` | Production definition |
| `\|` | Alternation |
| `*` | Zero or more |
| `+` | One or more |
| `?` | Optional (zero or one) |
| `( )` | Grouping |
| `"literal"` | Terminal string |
| `/regex/` | Terminal regular-expression class |

Production names use `lower_snake_case`. Terminal classes defined by regular expression use `UPPER_SNAKE_CASE`. Whitespace between tokens is significant where § 2 says it is; elsewhere, whitespace between tokens is ignored.

### Example 1.4.1

```ebnf
ingredients_step ::= "ingredients" ingredient+ NEWLINE
ingredient       ::= STRING | "!" STRING
```

This defines `ingredients_step` as the keyword `ingredients` followed by one or more ingredients and a newline. Each ingredient is either a bare string (include) or `!` followed by a string (exclude).

## 1.5. Precedence on grammar disagreements

Where the normative prose in chapters 03 or 04 disagrees with a production in Appendix A, Appendix A is authoritative. The prose is then a defect in the Standard and MUST be corrected to match Appendix A.

This rule applies only to grammar productions. Where a chapter's prose defines a semantic property not expressed in Appendix A (for example, the ordering constraint "`use` declarations MUST precede any recipe"), that prose is the normative source.

## 1.6. Amendment markers

When a normative clause is materially revised, an inline marker `[amended CS-NNNN]` MUST be appended to the clause, linking to the corresponding entry in `D-changes.mdx`. Additions do not require a marker. When a clause is removed, it is struck through (`~~...~~`) with an `[amended CS-NNNN]` marker; the struck-through form is retained for one release cycle and then purged.

## 1.7. Stable anchors

Every numbered chunk (chapter, section, subsection, example, production, note) has a stable markdown anchor so that cross-references survive renames. Anchors follow the pattern `s-N-M-K` for sections and `ex-N-M-K` for examples. Authors MUST NOT change an anchor without recording the change in `D-changes.mdx`.
```

- [ ] **Step 2: Commit**

```bash
git add docs/standard/01-notation.mdx
git commit -m "docs(standard): add § 1 Notation and conventions"
```

---

## Task 4 — Skeleton `02-lexical.mdx`

**Files:**
- Create: `docs/standard/02-lexical.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/02-lexical.mdx` with exactly this content:

````mdx
# 2. Lexical structure

> **Normative.** This chapter defines how a Cookfile's text is broken into tokens. The productions in § 3 and Appendix A consume the token classes defined here.

## 2.1. Source representation

Describes the Cookfile as a sequence of UTF-8 bytes, the line-oriented nature of lexing, and line-ending handling.

> **NORMATIVE-TODO (CS-stub-01).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, function `tokenize`.

## 2.2. Tokens

Describes the one-token-per-line model and enumerates the token classes the lexer produces.

> **NORMATIVE-TODO (CS-stub-02).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, enum `Token` and function `tokenize`.

## 2.3. Identifiers

Defines the `IDENTIFIER` character class and what identifier forms are accepted as names.

> **NORMATIVE-TODO (CS-stub-03).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, function `try_parse_var_decl` (for identifier validation) and the regular expression used for identifiers.

## 2.4. Keywords

Enumerates reserved keywords and the contexts in which they are reserved.

> **NORMATIVE-TODO (CS-stub-04).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, function `try_parse_var_decl` (for the reserved keyword list blocking variable names).

## 2.5. Strings

Defines the `STRING` token class: opening `"`, any character except `"`, closing `"`.

> **NORMATIVE-TODO (CS-stub-05).** See implementation: `cli/crates/cook-lang/src/lexer.rs` (string regex) and `cli/crates/cook-lang/src/cook_line.rs`, function `parse_single_quoted_string`.

## 2.6. Comments

Defines comment syntax (`#` to end-of-line) and the rules for where comments may appear.

> **NORMATIVE-TODO (CS-stub-06).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, handling of `#`-prefixed lines.

## 2.7. Line prefixes

Defines the `>`, `>{`, and `@` line prefixes and what they introduce.

> **NORMATIVE-TODO (CS-stub-07).** See implementation: `cli/crates/cook-lang/src/lexer.rs` (for `>` and `>{`) and `cli/crates/cook-lang/src/recipe.rs` Content-dispatch (for `@`).

## 2.8. Numbers

Defines the `NUMBER` token class, used by `test` steps for timeouts.

> **NORMATIVE-TODO (CS-stub-08).** See implementation: `cli/crates/cook-lang/src/cook_line.rs`, function `parse_test_timeout`.

## 2.9. Brace-balanced blocks

Defines how `>{ ... }` (Lua) and `{ ... }` (shell) blocks are collected, including brace counting through strings and comments.

> **NORMATIVE-TODO (CS-stub-09).** See implementation: `cli/crates/cook-lang/src/lua_block.rs`, functions `collect_lua_block` and `count_brace_delta`; and `cli/crates/cook-lang/src/shell_block.rs`.

## 2.10. Line classification cascade

Describes the priority order in which a single raw line is classified into a token variant.

> **NORMATIVE-TODO (CS-stub-10).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, function `tokenize` — the `if`/`else if` cascade.
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/02-lexical.mdx
git commit -m "docs(standard): add § 2 Lexical structure (skeleton)"
```

---

## Task 5 — Skeleton `03-syntactic-grammar.mdx`

**Files:**
- Create: `docs/standard/03-syntactic-grammar.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/03-syntactic-grammar.mdx` with exactly this content:

````mdx
# 3. Syntactic grammar

> **Normative.** This chapter walks through the productions of the Cookfile grammar in prose. The full formal grammar is in Appendix A, which is authoritative where it disagrees with the prose here (§ 1.5).

## 3.1. Grammar overview

Describes how a Cookfile is parsed top-down: a sequence of top-level items, each of which is a recipe, a config block, a use/import declaration, a variable declaration, a comment, or a blank line.

> **NORMATIVE-TODO (CS-stub-11).** See App. A for the authoritative productions and `cli/crates/cook-lang/src/lib.rs` function `parse` for the top-level loop.

## 3.2. Top-level ordering

Defines the rule that variable declarations, use declarations, import declarations, and config blocks MUST appear before any recipe.

> **NORMATIVE-TODO (CS-stub-12).** See implementation: `cli/crates/cook-lang/src/lib.rs`, the `seen_recipe` flag and the errors it triggers.

## 3.3. Variable declarations

Describes the `NAME "value"` top-level form.

> **NORMATIVE-TODO (CS-stub-13).** See implementation: `cli/crates/cook-lang/src/lexer.rs`, function `try_parse_var_decl`.

## 3.4. `use` declarations

Describes `use <name>` for loading a built-in or user-authored module.

> **NORMATIVE-TODO (CS-stub-14).** See implementation: `cli/crates/cook-lang/src/lexer.rs` (tokenization of `use`) and `cli/crates/cook-lang/src/lib.rs` (top-level dispatch).

## 3.5. `import` declarations

Describes `import <name> <path>` for aliasing another Cookfile as a module.

> **NORMATIVE-TODO (CS-stub-15).** See implementation: `cli/crates/cook-lang/src/lexer.rs` (tokenization of `import`) and `cli/crates/cook-lang/src/lib.rs` (top-level dispatch and duplicate detection).

## 3.6. Config blocks

Describes the `config [name] ... end` block whose body is free Lua code.

> **NORMATIVE-TODO (CS-stub-16).** See implementation: `cli/crates/cook-lang/src/recipe.rs`, function `parse_config_block_lua`.

## 3.7. Recipes

Describes recipe headers (explicit `recipe "name"` and implicit `name:`), dependency lists, and the body-until-`end` structure.

> **NORMATIVE-TODO (CS-stub-17).** See implementation: `cli/crates/cook-lang/src/recipe.rs`, function `parse_recipe`.

## 3.8. Step dispatch inside a recipe

Describes the priority cascade used to classify a `Content` token inside a recipe into one of: ingredients, cook, plate, test, module-call, interactive shell (`@`), or plain shell.

> **NORMATIVE-TODO (CS-stub-18).** See implementation: `cli/crates/cook-lang/src/recipe.rs`, function `parse_recipe`, the `Token::Content` arm.
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/03-syntactic-grammar.mdx
git commit -m "docs(standard): add § 3 Syntactic grammar (skeleton)"
```

---

## Task 6 — Skeleton `04-recipes.mdx`

**Files:**
- Create: `docs/standard/04-recipes.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/04-recipes.mdx` with exactly this content:

````mdx
# 4. Recipes and step kinds

> **Normative.** This chapter defines the structure of a recipe body and the meaning of each step kind.

## 4.1. Recipe header forms

Defines the explicit form (`recipe <name>[: deps...]`) and the implicit form (`<identifier>: [deps...]`).

> **NORMATIVE-TODO (CS-stub-19).** See implementation: `cli/crates/cook-lang/src/recipe.rs`, recipe-header parsing.

## 4.2. Dependency list

Defines the semantics of the `:` dependency-list suffix: a recipe listed as a dependency MUST exist; cycles are errors detected downstream.

> **NORMATIVE-TODO (CS-stub-20).** See implementation: `cli/crates/cook-lang/src/recipe.rs` (syntactic) and `cli/crates/cook-dag/` (cycle detection).

## 4.3. Ingredients

Defines the single `ingredients` line per recipe: one or more glob patterns, with `!"pattern"` denoting exclusion.

> **NORMATIVE-TODO (CS-stub-21).** See implementation: `cli/crates/cook-lang/src/cook_line.rs`, function `parse_ingredients_line`.

## 4.4. Step kinds — overview

Enumerates the seven step kinds that may appear in a recipe body: Shell, Interactive Shell, Lua line, Lua block, Cook, Plate, Test. Describes the dispatch rules from § 3.8.

> **NORMATIVE-TODO (CS-stub-22).** See implementation: `cli/crates/cook-lang/src/ast.rs`, enum `Step`.

## 4.5. `cook` step — single-output form

Defines `cook "out"`, `cook "out" using "cmd"`, `cook "out" using >{ ... }`, `cook "out" using { ... }`.

> **NORMATIVE-TODO (CS-stub-23).** See implementation: `cli/crates/cook-lang/src/cook_line.rs`, function `parse_cook_line`.

## 4.6. `cook` step — multi-output form

Defines `cook "a" "b" using { ... }` and `cook "a" "b" using >{ ... }`. Multi-output form requires a block-form `using` clause; `cook "a" "b" using "cmd"` is a parse error.

> **NORMATIVE-TODO (CS-stub-24).** See implementation: `cli/crates/cook-lang/src/cook_line.rs`, function `parse_cook_line` — the multi-output validation branch.

## 4.7. `plate` step

Defines `plate "cmd"` — a step that runs the given command for each output produced by a preceding `cook` step in the same recipe.

> **NORMATIVE-TODO (CS-stub-25).** See implementation: `cli/crates/cook-lang/src/recipe.rs` (syntax) and `cli/crates/cook-luagen/` (codegen).

## 4.8. `test` step

Defines `test "cmd" [timeout N] [should_fail]` — a step that runs the command and asserts its exit status and optional runtime bound.

> **NORMATIVE-TODO (CS-stub-26).** See implementation: `cli/crates/cook-lang/src/cook_line.rs`, functions `parse_test_command` and `parse_test_timeout`.

## 4.9. Lua steps — line and block

Defines `> expr` (single-line Lua) and `>{ ... }` (multi-line Lua block) steps.

> **NORMATIVE-TODO (CS-stub-27).** See implementation: `cli/crates/cook-lang/src/lexer.rs` (`LuaLine` / `LuaBlockOpen`) and `cli/crates/cook-lang/src/lua_block.rs`.

## 4.10. Shell steps — plain and interactive

Defines the default `shell_command` (any Content line not matching another step) and the `@`-prefixed `interactive_command`.

> **NORMATIVE-TODO (CS-stub-28).** See implementation: `cli/crates/cook-lang/src/recipe.rs`, the shell/interactive branches of Content dispatch.

## 4.11. Module-call steps

Defines the heuristic that classifies a Content line matching `IDENTIFIER . IDENTIFIER ...` as a Lua expression (module call) rather than a shell command. Module calls may span multiple lines when braces are unbalanced on the first line.

> **NORMATIVE-TODO (CS-stub-29).** See implementation: `cli/crates/cook-lang/src/recipe.rs`, functions `is_module_call` and `collect_module_call`.
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/04-recipes.mdx
git commit -m "docs(standard): add § 4 Recipes and step kinds (skeleton)"
```

---

## Task 7 — Skeleton `05-execution-model.mdx`

**Files:**
- Create: `docs/standard/05-execution-model.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/05-execution-model.mdx` with exactly this content:

````mdx
# 5. Execution model

> **Normative.** This chapter defines the observable execution semantics of a Cookfile. Implementation details (scheduler thread counts, hash functions, I/O strategies) are not normative; see `docs/architecture/` for the reference implementation's approach.

## 5.1. Two-phase execution

Defines the register phase and the execute phase. During the register phase, Lua code in the Cookfile runs once in capture mode; API calls record work without performing it. During the execute phase, the scheduler runs the recorded work in dependency order.

> **NORMATIVE-TODO (CS-stub-30).** See `docs/architecture/runtime.md` for the current reference description; normative text to be written from direct observation of `cli/crates/cook-register/` and `cli/crates/cook-luaotp/`.

## 5.2. Capture-mode semantics

Defines what Cook Lua API functions do during the register phase: they return handles / unit IDs synchronously and never block on external work.

> **NORMATIVE-TODO (CS-stub-31).** See implementation: `cli/crates/cook-register/`.

## 5.3. Step groups and within-recipe parallelism

Defines how steps in a recipe body are grouped, and the rule that steps in the same group may run in parallel while steps in different groups are ordered.

> **NORMATIVE-TODO (CS-stub-32).** See implementation: `cli/crates/cook-dag/` and `cli/crates/cook-luagen/`.

## 5.4. Cross-recipe dependencies

Defines how a recipe's `: "dep"` list creates explicit cross-recipe ordering.

> **NORMATIVE-TODO (CS-stub-33).** See implementation: `cli/crates/cook-dag/`.

## 5.5. Ingredient–serves matching

Defines the implicit-dependency rule: a recipe that serves a string automatically becomes a dependency of any recipe listing that string as an ingredient. Matching is exact string equality.

> **NORMATIVE-TODO (CS-stub-34).** See implementation: `cli/crates/cook-dag/`, ingredient/serves pairing logic.

## 5.6. Interactive step draining

Defines the rule that a step prefixed with `@` drains all concurrent work before running and runs on the main thread.

> **NORMATIVE-TODO (CS-stub-35).** See implementation: `cli/crates/cook-dag/` or the scheduler crate that handles interactive scheduling.

## 5.7. Cache semantics (abstract)

Defines, abstractly, when a unit is considered up-to-date and when it is re-run: a unit is re-run when any of its observable inputs (source-text-derived) differ from the last successful run. The hash function and storage format are implementation-defined.

> **NORMATIVE-TODO (CS-stub-36).** See implementation: `cli/crates/cook-cache/`.

## 5.8. Diagnostic ordering

Defines the rule that syntax errors are reported before semantic errors, and that parsing halts on the first syntax error.

> **NORMATIVE-TODO (CS-stub-37).** See implementation: `cli/crates/cook-lang/src/lib.rs` (syntax halts) and the engine for semantic ordering.
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/05-execution-model.mdx
git commit -m "docs(standard): add § 5 Execution model (skeleton)"
```

---

## Task 8 — Skeleton `06-cook-lua-api.mdx`

**Files:**
- Create: `docs/standard/06-cook-lua-api.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/06-cook-lua-api.mdx` with exactly this content:

````mdx
# 6. Cook Lua API

> **Normative.** This chapter defines the Cook API surface available to Lua code inside a Cookfile. Code encountered via `> expr`, `>{ ... }`, `using >{ ... }`, module bodies, and config blocks runs with this API in scope.

## 6.1. API surface overview

Enumerates the top-level Lua tables and functions the Cook runtime installs before any Cookfile code runs.

> **NORMATIVE-TODO (CS-stub-38).** See implementation: `cli/crates/cook-register/src/lib.rs` (entry points `register_fs_api`, `register_path_api`) and submodules registering specific API surfaces.

## 6.2. `cook.add_unit`

Defines the core work-unit registration API: arguments, return value, and capture-phase semantics.

> **NORMATIVE-TODO (CS-stub-39).** See implementation: `cli/crates/cook-register/` — the work-unit registration module.

## 6.3. Shell and Lua step helpers

Defines the Lua-side helpers that correspond to `shell` and `lua` step forms when invoked from inside a module body.

> **NORMATIVE-TODO (CS-stub-40).** See implementation: `cli/crates/cook-register/`.

## 6.4. Using-block globals

Defines the globals `input`, `output`, `inputs`, `outputs`, and `input_N` that are bound inside a `using >{ ... }` or `using { ... }` block at execute time.

> **NORMATIVE-TODO (CS-stub-41).** See implementation: `cli/crates/cook-luagen/` — codegen for `BlockStep` binding setup.

## 6.5. Filesystem helpers (`fs.*`)

Defines the `fs.*` functions available in Lua contexts.

> **NORMATIVE-TODO (CS-stub-42).** See implementation: `cli/crates/cook-register/src/lib.rs` — `register_fs_api` and its submodule.

## 6.6. Path helpers (`path.*`)

Defines the `path.*` functions available in Lua contexts.

> **NORMATIVE-TODO (CS-stub-43).** See implementation: `cli/crates/cook-register/src/lib.rs` — `register_path_api` and its submodule.

## 6.7. Placeholder substitutions in shell strings

Defines the `{in}`, `{out}`, `{all}`, `{stem}`, and related placeholders available in shell using-strings.

> **NORMATIVE-TODO (CS-stub-44).** See implementation: `cli/crates/cook-luagen/` — placeholder expansion in shell `using` clauses.
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/06-cook-lua-api.mdx
git commit -m "docs(standard): add § 6 Cook Lua API (skeleton)"
```

---

## Task 9 — Skeleton `07-modules.mdx`

**Files:**
- Create: `docs/standard/07-modules.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/07-modules.mdx` with exactly this content:

````mdx
# 7. Modules

> **Normative.** This chapter defines the module system: how modules are named, loaded, resolved, and what a module is expected to expose.

## 7.1. Module concept

Describes a module as a named bundle of Lua code that augments the Cook runtime (e.g., adding `cpp.bin`, `cpp.compile_commands`). A module is either built-in, imported by path, or resolved from `cook_modules/`.

> **NORMATIVE-TODO (CS-stub-45).** See implementation: `cli/crates/cook-register/` — module registration surfaces.

## 7.2. `use <name>` declarations

Defines how `use foo` resolves: first as a built-in, then as a package in `cook_modules/`.

> **NORMATIVE-TODO (CS-stub-46).** See implementation: the module-resolution logic used by `cook-register` / `cook-engine`.

## 7.3. `import <name> <path>` declarations

Defines how `import backend ./path/to/other` aliases another Cookfile as a local module under the given identifier.

> **NORMATIVE-TODO (CS-stub-47).** See implementation: `cli/crates/cook-engine/` — import resolution.

## 7.4. `cook_modules/` resolution

Defines the lookup algorithm for `cook_modules/<name>/`: directory form, package.json contents, entry points.

> **NORMATIVE-TODO (CS-stub-48).** See implementation: `cli/crates/cook-engine/` — `cook_modules/` lookup.

## 7.5. Module authoring contract

Defines what a conforming module must expose, how it registers step kinds, and what globals it may introduce.

> **NORMATIVE-TODO (CS-stub-49).** See implementation: examples in `examples/*/cook_modules/` and the `cook-register` registration surface.

## 7.6. Duplicate and cycle detection

Defines the rule that duplicate `import` names are errors, and that import cycles are errors.

> **NORMATIVE-TODO (CS-stub-50).** See implementation: `cli/crates/cook-lang/src/lib.rs` (duplicate import detection) and the engine (cycle detection).
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/07-modules.mdx
git commit -m "docs(standard): add § 7 Modules (skeleton)"
```

---

## Task 10 — Reconnaissance for Appendix A (the normative grammar)

This is a reading task, no file changes. The goal is to confirm every production the EBNF in Task 11 will encode by directly consulting the Rust parser source. The executor MUST read these files in order and take notes before writing Appendix A.

- [ ] **Step 1: Read the lexer**

Read `cli/crates/cook-lang/src/lexer.rs` end to end. In particular, note:

- The `Token` enum variants and when each is emitted.
- The reserved-keyword list inside `try_parse_var_decl`.
- Handling of `use` and `import` tokens (lines, expected arguments).
- The regex used for `IDENTIFIER` and `STRING`.
- The brace-counting support for `LuaBlockOpen`.

- [ ] **Step 2: Read the AST**

Read `cli/crates/cook-lang/src/ast.rs` end to end. In particular, note:

- Every field of `Cookfile`: `vars`, `config_blocks`, `recipes`, `uses`, `imports`.
- Every field of `Recipe`: `name`, `deps`, `ingredients`, `excludes`, `steps`, `line`.
- Every variant of `Step`: `Shell { interactive }`, `Lua`, `LuaBlock`, `Cook`, `Plate`, `Test`.
- `CookStep { outputs, using_clause }` and `UsingClause { Shell, LuaBlock, ShellBlock }`.
- `PlateStep`, `TestStep { command, timeout, should_fail }`, `ConfigBlock { name, body, line }`.
- `UseStatement { module_name, line }`, `ImportDecl { name, path, line }`.

- [ ] **Step 3: Read the top-level parser**

Read `cli/crates/cook-lang/src/lib.rs`, function `parse`. Note:

- The order in which token classes dispatch.
- The `seen_recipe` guard and which declarations it gates.
- Handling of `use` / `import` tokens and duplicate-import detection.

- [ ] **Step 4: Read the recipe parser**

Read `cli/crates/cook-lang/src/recipe.rs`, functions `parse_recipe`, `parse_config_block_lua`, `is_module_call`, `collect_module_call`. Note:

- The `Content` dispatch cascade: `ingredients`, `cook`, `plate`, `test`, module-call, `@`, shell.
- How `ingredients` handles duplicate lines and returns (includes, excludes).
- How `test` consumes optional `timeout N` and `should_fail`.
- How `is_module_call` identifies a module call syntactically.

- [ ] **Step 5: Read the cook-line and block helpers**

Read `cli/crates/cook-lang/src/cook_line.rs`, functions `parse_cook_line`, `parse_ingredients_line`, `parse_single_quoted_string`, `parse_test_command`, `parse_test_timeout`. Also read `cli/crates/cook-lang/src/lua_block.rs` and `cli/crates/cook-lang/src/shell_block.rs`. Note:

- How `parse_cook_line` handles single-output vs multi-output and which `using` forms are accepted for each.
- The brace-balance algorithm and its handling of strings and comments.
- The line-granularity of shell blocks vs Lua blocks.

- [ ] **Step 6: Do NOT consult `tree-sitter-cook/grammar.js`**

Per the source-of-truth rule (plan header and `project_rust_parser_source_of_truth.md`): the tree-sitter grammar is known stale. Reading it at this stage will mislead. The grammar in Appendix A is derived solely from the Rust parser. A follow-up plan (`CS-0002`) brings `grammar.js` into conformance.

- [ ] **Step 7: Commit (no-op)**

Nothing to commit; this was a reconnaissance task. Proceed to Task 11.

---

## Task 11 — Write Appendix A (the normative grammar)

**Files:**
- Create: `docs/standard/A-grammar.mdx`

- [ ] **Step 1: Write Appendix A**

Create `docs/standard/A-grammar.mdx` with the content below. Before committing, confirm each production against the notes from Task 10. If a production does not match the Rust parser's observable behavior, the production is wrong — correct it to match the parser.

````mdx
# Appendix A. Grammar (normative)

> **Normative.** This appendix contains the full formal grammar of the Cookfile language in the W3C-style EBNF variant defined in § 1.4. Where this appendix disagrees with the prose of § 3 or § 4, this appendix is authoritative (§ 1.5).

## A.1. Top level

```ebnf
cookfile              ::= toplevel_item*
toplevel_item         ::= recipe
                       | config_block
                       | use_declaration
                       | import_declaration
                       | variable_declaration
                       | comment
                       | NEWLINE
```

## A.2. Top-level declarations

```ebnf
variable_declaration  ::= IDENTIFIER STRING NEWLINE
use_declaration       ::= "use" name NEWLINE
import_declaration    ::= "import" IDENTIFIER path NEWLINE
config_block          ::= "config" name? NEWLINE config_body "end" NEWLINE
config_body           ::= LUA_SOURCE
```

**Ordering (normative).** `variable_declaration`, `use_declaration`, `import_declaration`, and `config_block` MUST appear before the first `recipe`. A conforming implementation MUST reject a Cookfile that places any of these after a recipe.

**Identifiers blocked from variable_declaration.** The identifiers `recipe`, `config`, `end`, `ingredients`, `cook`, `plate`, `taste`, `using`, `test`, `use`, `import` MUST NOT be used as the left-hand side of a `variable_declaration`.

## A.3. Recipes

```ebnf
recipe                ::= recipe_header NEWLINE recipe_body "end" NEWLINE?
recipe_header         ::= explicit_header
                       | implicit_header
explicit_header       ::= "recipe" name (":" dependency_list)?
implicit_header       ::= IDENTIFIER ":" dependency_list?
dependency_list       ::= name+
recipe_body           ::= recipe_item*
recipe_item           ::= ingredients_step
                       | cook_step
                       | plate_step
                       | test_step
                       | lua_line
                       | lua_block
                       | module_call
                       | interactive_command
                       | shell_command
                       | comment
                       | NEWLINE
```

**At-most-one-ingredients rule.** A `recipe_body` MUST contain at most one `ingredients_step`. A conforming implementation MUST reject a recipe with two or more `ingredients_step` items.

## A.4. Steps

```ebnf
ingredients_step      ::= "ingredients" ingredient+ NEWLINE
ingredient            ::= STRING            /* include */
                       | "!" STRING          /* exclude */

cook_step             ::= "cook" STRING+ using_clause? NEWLINE
using_clause          ::= "using" (STRING | inline_lua_block | shell_block)
inline_lua_block      ::= ">{" LUA_BLOCK_CONTENT "}"
shell_block           ::= "{" SHELL_BLOCK_CONTENT "}"

plate_step            ::= "plate" STRING NEWLINE

test_step             ::= "test" STRING ("timeout" NUMBER)? "should_fail"? NEWLINE

lua_line              ::= ">" LUA_LINE_CONTENT NEWLINE
lua_block             ::= ">{" LUA_BLOCK_CONTENT "}" NEWLINE

interactive_command   ::= "@" SHELL_LINE_CONTENT NEWLINE

module_call           ::= MODULE_CALL_TEXT
                          /* MODULE_CALL_TEXT is any Content line whose first
                             token matches /[A-Za-z_][A-Za-z0-9_]*\.[A-Za-z_]/
                             and whose braces balance (possibly across
                             subsequent lines per § 2.9). See § 4.11. */

shell_command         ::= SHELL_LINE_CONTENT NEWLINE
                          /* Any Content line not matching a preceding
                             alternative. See § 3.8. */
```

**Multi-output cook-step rule.** When `cook_step` has two or more `STRING` patterns before `using`, the `using_clause` MUST be `inline_lua_block` or `shell_block`. A conforming implementation MUST reject the form `cook "a" "b" using STRING`.

**Declaration-only cook step.** A `cook_step` whose `using_clause` is absent is a declaration — it announces the output without providing a build command inline. Its build is assumed to be provided by preceding Lua registrations or a later amendment.

**Step-dispatch priority (normative).** A single `Content` line inside a recipe body is dispatched in this order, with the first match winning:

1. Prefix `ingredients` + separator → `ingredients_step`.
2. Prefix `cook` + separator → `cook_step`.
3. Prefix `plate` + separator → `plate_step`.
4. Prefix `test` + separator → `test_step`.
5. Matches the module-call pattern (§ 4.11) → `module_call`.
6. First character is `@` and the remainder is non-empty → `interactive_command`.
7. Otherwise → `shell_command`.

## A.5. Primitives

```ebnf
name                  ::= IDENTIFIER | STRING
IDENTIFIER            ::= /[a-zA-Z_][a-zA-Z0-9_.\-]*/
STRING                ::= /"[^"]*"/
NUMBER                ::= /[0-9]+/
path                  ::= /[^\s\n]+/
comment               ::= /#[^\n]*/ NEWLINE
NEWLINE               ::= /\n/
LUA_LINE_CONTENT      ::= /[^{\n][^\n]*/   /* first char is not '{' */
LUA_BLOCK_CONTENT     ::= BRACE_BALANCED   /* see § 2.9 */
SHELL_LINE_CONTENT    ::= /[^\n]+/
SHELL_BLOCK_CONTENT   ::= BRACE_BALANCED   /* see § 2.9 */
LUA_SOURCE            ::= /* any number of lines up to the matching "end" */
BRACE_BALANCED        ::= /* text in which `{` and `}` counted outside of
                             comments and strings balance to zero; see § 2.9 */
MODULE_CALL_TEXT      ::= /* see § 4.11 */
```

**Lexical note.** `LUA_LINE_CONTENT`'s restriction that its first character is not `{` disambiguates `>` (lua line) from `>{` (lua block open).
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/A-grammar.mdx
git commit -m "docs(standard): add App. A — normative grammar"
```

---

## Task 12 — Skeleton `B-rationale.mdx`

**Files:**
- Create: `docs/standard/B-rationale.mdx`

- [ ] **Step 1: Write the skeleton**

Create `docs/standard/B-rationale.mdx` with exactly this content:

```mdx
# Appendix B. Rationale (informative)

> **Informative.** This appendix records the reasoning behind design decisions in the Standard. It is non-binding.

## B.0. On § 0 Introduction

_To be filled in._

## B.1. On § 1 Notation and conventions

_To be filled in._

## B.2. On § 2 Lexical structure

_To be filled in._

## B.3. On § 3 Syntactic grammar

_To be filled in._

## B.4. On § 4 Recipes and step kinds

_To be filled in._

## B.5. On § 5 Execution model

_To be filled in._

## B.6. On § 6 Cook Lua API

_To be filled in._

## B.7. On § 7 Modules

_To be filled in._

## B.A. On Appendix A

_To be filled in._
```

- [ ] **Step 2: Commit**

```bash
git add docs/standard/B-rationale.mdx
git commit -m "docs(standard): add App. B Rationale (skeleton)"
```

---

## Task 13 — Seed `C-examples.mdx`

**Files:**
- Create: `docs/standard/C-examples.mdx`

- [ ] **Step 1: Write the seed examples**

Create `docs/standard/C-examples.mdx` with the content below. Each example is drawn from the existing `examples/` directory and is annotated with the Standard sections it exercises.

````mdx
# Appendix C. Worked examples (informative)

> **Informative.** This appendix presents worked Cookfile examples, each annotated with the sections of the Standard it exercises. Examples are non-binding; they illustrate the normative rules defined in chapters 2–7 and Appendix A.

## C.1. Multi-output cook step (from `examples/multi-output/Cookfile`)

```cook
recipe generate
    ingredients "src/*.rs"
    cook "staging/out.js" "staging/out.wasm" using {
        ./fake-generator.sh
        mkdir -p staging
        cp pkg/out.js staging/out.js
        cp pkg/out.wasm staging/out.wasm
    }
end
```

Exercises: § 4.1 (explicit header), § 4.3 (ingredients), § 4.6 (multi-output cook with `using { ... }` shell block), App. A.3, App. A.4.

## C.2. Recipe with cross-recipe dependency (from `examples/cross-recipe-deps/Cookfile`)

```cook
recipe tools: colorize wordcount reverse
end

recipe play: tools
    @mkdir -p .cook/scratch
    @printf 'colorize\nwordcount\nreverse\n' | fzf --prompt='tool> ' > .cook/scratch/tool
    @find data -type f | fzf --prompt='file> ' --preview='cat {}' > .cook/scratch/file
    @./build/bin/$(cat .cook/scratch/tool) < "$(cat .cook/scratch/file)"
end
```

Exercises: § 4.1 (explicit header with deps), § 4.2 (dependency list), § 4.10 (interactive `@` shell commands), App. A.3, App. A.4.

## C.3. Module use and call (from `examples/fzf-picker/Cookfile`)

```cook
use cpp

config
    env.CFLAGS = "-O2 -Wall"
end

recipe colorize
    cpp.bin("colorize", {
        sources = { "src/colorize.c" },
    })
end
```

Exercises: § 3.4 (`use` declaration), § 3.6 (config block with Lua body), § 3.8 (step dispatch), § 4.11 (module-call step), App. A.2, App. A.4.

## C.4. Using-block Lua with multiple outputs (from `examples/multi-output/Cookfile`)

```cook
recipe build_lua
    ingredients "src/*.rs"
    cook "staging-lua/out.js" "staging-lua/out.wasm" using >{
        os.execute("mkdir -p staging-lua")
        local sources = table.concat(inputs, " ")
        local js = io.open(outputs[1], "w")
        js:write("// generated from " .. sources .. "\n")
        js:close()
        local wasm = io.open(outputs[2], "w")
        wasm:write("BINARY\n")
        wasm:close()
    }
end
```

Exercises: § 4.6 (multi-output cook with `using >{ ... }` Lua block), § 6.4 (using-block globals `inputs` / `outputs`), App. A.4.
````

- [ ] **Step 2: Commit**

```bash
git add docs/standard/C-examples.mdx
git commit -m "docs(standard): add App. C Worked examples (seed)"
```

---

## Task 14 — Initialize `D-changes.mdx`

**Files:**
- Create: `docs/standard/D-changes.mdx`

- [ ] **Step 1: Write the changelog**

Create `docs/standard/D-changes.mdx` with the following content:

```mdx
# Appendix D. Changes (informative)

> **Informative.** This appendix is the chronological changelog of amendments to the Cook Standard. Each entry has a stable `CS-NNNN` ID, a one-line summary, the list of sections affected, and the commit / PR reference.

## CS-0001 — Cook Standard v0.1 established

**Date:** 2026-04-22
**Sections affected:** entire Standard (establishment).
**Summary:** Initial skeleton established. Chapters 0, 1 and Appendix A are fully written; chapters 2–7 ship as numbered outlines with `NORMATIVE-TODO` stubs referencing the Rust parser as the de-facto authority. Conformance corpus and `cook-lang` integration harness are wired. `tree-sitter-cook` is known stale and is NOT wired for conformance in this change; see `CS-0002`.
**Reference:** Established in the commit that introduces `docs/standard/`.

## CS-0002 — Planned: tree-sitter-cook conformance audit

**Date:** planned.
**Sections affected:** none in the Standard itself. This entry is a forward reference to a follow-up plan that will audit `tree-sitter-cook/grammar.js` against the Standard and bring it into conformance, at which point a tree-sitter harness against the same corpus will be added alongside the existing `cook-lang` harness.
**Summary:** Tree-sitter currently lags behind the Rust parser on several constructs: multi-output cook steps, shell-block `using` clauses, `test` steps, ingredient excludes (`!"..."`), Lua-code config blocks, and the module-call heuristic. The follow-up plan will: (a) enumerate divergences, (b) update `grammar.js` and its externals, (c) wire a tree-sitter corpus against `docs/standard/conformance/`, (d) verify both implementations agree.
**Reference:** to be filled when the plan is written.
```

- [ ] **Step 2: Commit**

```bash
git add docs/standard/D-changes.mdx
git commit -m "docs(standard): add App. D Changes with CS-0001 and CS-0002 entries"
```

---

## Task 15 — Write `README.mdx` with table of contents

**Files:**
- Create: `docs/standard/README.mdx`
- Delete: `docs/standard/.gitkeep`

- [ ] **Step 1: Write the README**

Create `docs/standard/README.mdx` with the content below:

```mdx
# The Cook Standard

**Status:** Draft — head-of-main lockstep, pre-1.0.
**Source of truth:** the Rust parser in `cli/crates/cook-lang/`.

The Cook Standard is the authoritative specification of the Cookfile language. It is maintained in lockstep with the `main` branch: any change that affects language surface MUST be reflected in the Standard in the same commit that changes the implementation.

The Standard is a living document. Chapters are filled in incrementally; today, chapters 0, 1, and Appendix A are fully written, while chapters 2 through 7 are numbered outlines with `NORMATIVE-TODO` stubs referencing the de-facto authoritative implementation (the Rust parser). A language change that touches a stubbed section MUST fill that section's normative prose as part of the same change.

## Table of contents

### Normative

- [§ 0 — Introduction](00-introduction.mdx)
- [§ 1 — Notation and conventions](01-notation.mdx)
- [§ 2 — Lexical structure](02-lexical.mdx) (skeleton)
- [§ 3 — Syntactic grammar](03-syntactic-grammar.mdx) (skeleton)
- [§ 4 — Recipes and step kinds](04-recipes.mdx) (skeleton)
- [§ 5 — Execution model](05-execution-model.mdx) (skeleton)
- [§ 6 — Cook Lua API](06-cook-lua-api.mdx) (skeleton)
- [§ 7 — Modules](07-modules.mdx) (skeleton)
- [App. A — Grammar](A-grammar.mdx)

### Informative

- [App. B — Rationale](B-rationale.mdx) (skeleton)
- [App. C — Worked examples](C-examples.mdx)
- [App. D — Changes](D-changes.mdx)

### Conformance

- [`conformance/positive/`](conformance/positive/) — Cookfiles that conforming implementations MUST accept.
- [`conformance/negative/`](conformance/negative/) — Cookfiles that conforming implementations MUST reject.

## How to change the Standard

See `CONTRIBUTING.md` at the repo root. In brief:

1. Any PR affecting Cookfile surface syntax, execution semantics, the Cook Lua API, or the module system MUST update `docs/standard/` in the same PR.
2. Add one entry to [`D-changes.mdx`](D-changes.mdx) with a new stable `CS-NNNN` ID.
3. If the grammar changes, update [`A-grammar.mdx`](A-grammar.mdx).
4. If the change is observable from a Cookfile, add at least one case to `conformance/positive/` or `conformance/negative/`.

The repo ships a `.githooks/pre-commit` hook that checks this discipline locally. Install it with:

```bash
git config core.hooksPath .githooks
```

## Reading order

First-time readers: read § 0, § 1, and Appendix A in that order. Skim App. C for worked examples. Return to chapters 2–7 as their prose is filled in; for now, the Rust parser is the de-facto authority for constructs whose Standard section is stubbed.
```

- [ ] **Step 2: Remove the placeholder**

```bash
rm docs/standard/.gitkeep
```

- [ ] **Step 3: Commit**

```bash
git add docs/standard/README.mdx docs/standard/.gitkeep
git commit -m "docs(standard): add README with table of contents"
```

---

## Task 16 — Seed the positive conformance corpus

**Files:** (create, all under `docs/standard/conformance/positive/`)

- `001-empty-recipe/Cookfile`
- `001-empty-recipe/parse.txt`
- `001-empty-recipe/notes.md`
- `002-shell-step/Cookfile`
- `002-shell-step/parse.txt`
- `002-shell-step/notes.md`
- `003-interactive-shell/Cookfile`
- `003-interactive-shell/parse.txt`
- `003-interactive-shell/notes.md`
- `004-ingredients-with-exclude/Cookfile`
- `004-ingredients-with-exclude/parse.txt`
- `004-ingredients-with-exclude/notes.md`
- `005-cook-single-output-shell/Cookfile`
- `005-cook-single-output-shell/parse.txt`
- `005-cook-single-output-shell/notes.md`
- `006-cook-multi-output-shell-block/Cookfile`
- `006-cook-multi-output-shell-block/parse.txt`
- `006-cook-multi-output-shell-block/notes.md`
- `007-cook-multi-output-lua-block/Cookfile`
- `007-cook-multi-output-lua-block/parse.txt`
- `007-cook-multi-output-lua-block/notes.md`
- `008-lua-line-and-block/Cookfile`
- `008-lua-line-and-block/parse.txt`
- `008-lua-line-and-block/notes.md`
- `009-test-step/Cookfile`
- `009-test-step/parse.txt`
- `009-test-step/notes.md`
- `010-use-and-module-call/Cookfile`
- `010-use-and-module-call/parse.txt`
- `010-use-and-module-call/notes.md`

The `parse.txt` format is a human-readable AST summary used by the Rust harness (Task 18) for comparison. It is stable, deterministic, and captures the AST shape without committing to a JSON serialization format. The format is:

```
Cookfile
  uses: [...]
  imports: [...]
  vars: [...]
  config_blocks: [...]
  recipes:
    Recipe name=<name> line=<N>
      deps: [...]
      ingredients: [...]
      excludes: [...]
      steps:
        <step-line>
        <step-line>
        ...
```

Each `<step-line>` is one of:

- `Shell interactive=<bool> command=<repr>`
- `Lua code=<repr>`
- `LuaBlock code=<repr>`
- `Cook outputs=<repr-list> using=<None|Shell(repr)|LuaBlock(repr)|ShellBlock(repr-list)>`
- `Plate command=<repr>`
- `Test command=<repr> timeout=<None|u64> should_fail=<bool>`

Where `<repr>` is the string wrapped in double quotes with backslash-escapes for `"`, `\`, and `\n`. `<repr-list>` is `[<repr>, <repr>, ...]`.

All 10 cases below; each has the same three-file structure.

- [ ] **Step 1: Create case 001 — empty recipe**

`docs/standard/conformance/positive/001-empty-recipe/Cookfile`:

```
recipe "build"
end
```

`docs/standard/conformance/positive/001-empty-recipe/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="build" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
```

`docs/standard/conformance/positive/001-empty-recipe/notes.md`:

```md
Pins the minimal recipe form: `recipe "name"` header, empty body, `end`. Exercises § 3.7 and § 4.1.
```

- [ ] **Step 2: Create case 002 — shell step**

`docs/standard/conformance/positive/002-shell-step/Cookfile`:

```
recipe "clean"
    rm -rf build
end
```

`docs/standard/conformance/positive/002-shell-step/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="clean" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
        Shell interactive=false command="rm -rf build"
```

`docs/standard/conformance/positive/002-shell-step/notes.md`:

```md
Pins that any non-keyword-prefixed Content line inside a recipe becomes a plain shell step with `interactive=false`. Exercises § 4.10 and App. A.4 (dispatch step 7).
```

- [ ] **Step 3: Create case 003 — interactive shell**

`docs/standard/conformance/positive/003-interactive-shell/Cookfile`:

```
recipe "play"
    @./run
end
```

`docs/standard/conformance/positive/003-interactive-shell/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="play" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
        Shell interactive=true command="./run"
```

`docs/standard/conformance/positive/003-interactive-shell/notes.md`:

```md
Pins the `@` prefix: it is stripped and the step is Shell with `interactive=true`. Exercises § 4.10 and App. A.4 (dispatch step 6).
```

- [ ] **Step 4: Create case 004 — ingredients with exclude**

`docs/standard/conformance/positive/004-ingredients-with-exclude/Cookfile`:

```
recipe "pack"
    ingredients "src/*.c" !"src/excluded.c"
end
```

`docs/standard/conformance/positive/004-ingredients-with-exclude/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="pack" line=1
      deps: []
      ingredients: ["src/*.c"]
      excludes: ["src/excluded.c"]
      steps:
```

`docs/standard/conformance/positive/004-ingredients-with-exclude/notes.md`:

```md
Pins the `ingredients` line with a bare include and a `!`-prefixed exclude. Exercises § 4.3 and App. A.4 (ingredient rule).
```

- [ ] **Step 5: Create case 005 — cook single-output with shell using**

`docs/standard/conformance/positive/005-cook-single-output-shell/Cookfile`:

```
recipe "compile"
    ingredients "src/main.c"
    cook "build/main.o" using "gcc -c {in} -o {out}"
end
```

`docs/standard/conformance/positive/005-cook-single-output-shell/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="compile" line=1
      deps: []
      ingredients: ["src/main.c"]
      excludes: []
      steps:
        Cook outputs=["build/main.o"] using=Shell("gcc -c {in} -o {out}")
```

`docs/standard/conformance/positive/005-cook-single-output-shell/notes.md`:

```md
Pins single-output `cook` with a bare-string `using` clause. Exercises § 4.5 and App. A.4.
```

- [ ] **Step 6: Create case 006 — cook multi-output with shell block**

`docs/standard/conformance/positive/006-cook-multi-output-shell-block/Cookfile`:

```
recipe "gen"
    cook "out/a.js" "out/a.wasm" using {
        echo one
        echo two
    }
end
```

`docs/standard/conformance/positive/006-cook-multi-output-shell-block/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="gen" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
        Cook outputs=["out/a.js", "out/a.wasm"] using=ShellBlock(["echo one", "echo two"])
```

`docs/standard/conformance/positive/006-cook-multi-output-shell-block/notes.md`:

```md
Pins multi-output `cook` with `using { ... }` shell block form. Exercises § 4.6 and App. A.4 (multi-output cook rule).
```

- [ ] **Step 7: Create case 007 — cook multi-output with Lua block**

`docs/standard/conformance/positive/007-cook-multi-output-lua-block/Cookfile`:

```
recipe "gen_lua"
    cook "out/a.js" "out/a.wasm" using >{
        local f = io.open(outputs[1], "w"); f:write("js"); f:close()
        local g = io.open(outputs[2], "w"); g:write("wasm"); g:close()
    }
end
```

`docs/standard/conformance/positive/007-cook-multi-output-lua-block/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="gen_lua" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
        Cook outputs=["out/a.js", "out/a.wasm"] using=LuaBlock("        local f = io.open(outputs[1], \"w\"); f:write(\"js\"); f:close()\n        local g = io.open(outputs[2], \"w\"); g:write(\"wasm\"); g:close()")
```

`docs/standard/conformance/positive/007-cook-multi-output-lua-block/notes.md`:

```md
Pins multi-output `cook` with `using >{ ... }` Lua block form. Exercises § 4.6 and § 6.4.
```

> **Note on `parse.txt` for Lua blocks:** The `code` string is whatever the Rust parser captures between `>{` and `}`. Before this case is committed, the executor MUST run the Rust parser on this Cookfile and copy the actual captured string into `parse.txt`, including any leading whitespace. This avoids guessing how leading-indentation handling is implemented.

- [ ] **Step 8: Create case 008 — Lua line and block**

`docs/standard/conformance/positive/008-lua-line-and-block/Cookfile`:

```
recipe "lua_step"
    > print("hello")
    >{
        local x = 1
        print(x + 1)
    }
end
```

`docs/standard/conformance/positive/008-lua-line-and-block/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="lua_step" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="print(\"hello\")"
        LuaBlock code="        local x = 1\n        print(x + 1)"
```

`docs/standard/conformance/positive/008-lua-line-and-block/notes.md`:

```md
Pins the single-line `>` form and the multi-line `>{ ... }` form as Lua and LuaBlock steps, respectively. Before committing, run the parser on this Cookfile and verify the `LuaBlock code=` string matches what the parser actually captures (including leading indentation). Exercises § 4.9.
```

- [ ] **Step 9: Create case 009 — test step**

`docs/standard/conformance/positive/009-test-step/Cookfile`:

```
recipe "verify"
    test "./check" timeout 30
    test "./mustfail" should_fail
end
```

`docs/standard/conformance/positive/009-test-step/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="verify" line=1
      deps: []
      ingredients: []
      excludes: []
      steps:
        Test command="./check" timeout=Some(30) should_fail=false
        Test command="./mustfail" timeout=None should_fail=true
```

`docs/standard/conformance/positive/009-test-step/notes.md`:

```md
Pins `test` with `timeout N` and `should_fail` flags. Exercises § 4.8.
```

- [ ] **Step 10: Create case 010 — use and module call**

`docs/standard/conformance/positive/010-use-and-module-call/Cookfile`:

```
use cpp

recipe "colorize"
    cpp.bin("colorize", { sources = { "src/colorize.c" } })
end
```

`docs/standard/conformance/positive/010-use-and-module-call/parse.txt`:

```
Cookfile
  uses: [UseStatement module_name="cpp" line=1]
  imports: []
  vars: []
  config_blocks: []
  recipes:
    Recipe name="colorize" line=3
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="cpp.bin(\"colorize\", { sources = { \"src/colorize.c\" } })"
```

`docs/standard/conformance/positive/010-use-and-module-call/notes.md`:

```md
Pins the `use cpp` top-level declaration and the module-call classification: `cpp.bin(...)` inside a recipe body becomes a `Lua` step (not a `Shell` step). Exercises § 3.4, § 4.11, and App. A.4 (dispatch step 5).
```

- [ ] **Step 11: Commit**

```bash
git add docs/standard/conformance/positive/
git commit -m "docs(standard): seed positive conformance corpus (10 cases)"
```

---

## Task 17 — Seed the negative conformance corpus

**Files:** (create, all under `docs/standard/conformance/negative/`)

- `001-unterminated-string/{Cookfile,error.txt,notes.md}`
- `002-bare-at-prefix/{Cookfile,error.txt,notes.md}`
- `003-use-after-recipe/{Cookfile,error.txt,notes.md}`
- `004-duplicate-ingredients/{Cookfile,error.txt,notes.md}`
- `005-multi-output-using-string/{Cookfile,error.txt,notes.md}`

The `error.txt` format is a single line containing a substring that MUST appear somewhere in the Rust parser's error message for the given Cookfile. The substring is the **normative diagnostic class indicator**; exact wording is implementation-defined (§ 7.2). The test harness (Task 18) checks `contains` only.

- [ ] **Step 1: Create case 001 — unterminated string**

`docs/standard/conformance/negative/001-unterminated-string/Cookfile`:

```
recipe "build
end
```

`docs/standard/conformance/negative/001-unterminated-string/error.txt`:

```
unterminated string
```

`docs/standard/conformance/negative/001-unterminated-string/notes.md`:

```md
Diagnostic class: unterminated string literal. Rejected at lex time. Exercises § 2.5 and § 7 conformance item 2.
```

- [ ] **Step 2: Create case 002 — bare `@` prefix**

`docs/standard/conformance/negative/002-bare-at-prefix/Cookfile`:

```
recipe "broken"
    @
end
```

`docs/standard/conformance/negative/002-bare-at-prefix/error.txt`:

```
empty
```

`docs/standard/conformance/negative/002-bare-at-prefix/notes.md`:

```md
Diagnostic class: `@` must be followed by non-empty content. The substring "empty" is chosen to match the Rust parser's wording (`empty @ line` or similar). If the Rust parser's message uses a different word, update this file to match the normative class — but the class itself ("empty interactive command") is normative.
```

> **Before committing this case:** run the Rust parser on the Cookfile and capture the actual error message. If "empty" does not appear in the message, substitute a word that does (e.g., "interactive", "@"). The substring is a conformance class indicator; whatever word reliably appears in the Rust parser's current error for this class is appropriate.

- [ ] **Step 3: Create case 003 — `use` after recipe**

`docs/standard/conformance/negative/003-use-after-recipe/Cookfile`:

```
recipe "a"
end

use cpp
```

`docs/standard/conformance/negative/003-use-after-recipe/error.txt`:

```
before recipes
```

`docs/standard/conformance/negative/003-use-after-recipe/notes.md`:

```md
Diagnostic class: ordering rule violation — `use` must precede the first recipe. Exercises § 3.2 and App. A.2 ordering rule.
```

- [ ] **Step 4: Create case 004 — duplicate ingredients line**

`docs/standard/conformance/negative/004-duplicate-ingredients/Cookfile`:

```
recipe "pack"
    ingredients "a.c"
    ingredients "b.c"
end
```

`docs/standard/conformance/negative/004-duplicate-ingredients/error.txt`:

```
duplicate
```

`docs/standard/conformance/negative/004-duplicate-ingredients/notes.md`:

```md
Diagnostic class: at-most-one `ingredients` line per recipe. Exercises § 4.3 and App. A.3.
```

- [ ] **Step 5: Create case 005 — multi-output with bare-string `using`**

`docs/standard/conformance/negative/005-multi-output-using-string/Cookfile`:

```
recipe "bad"
    cook "a.out" "b.out" using "echo no"
end
```

`docs/standard/conformance/negative/005-multi-output-using-string/error.txt`:

```
multi-output
```

`docs/standard/conformance/negative/005-multi-output-using-string/notes.md`:

```md
Diagnostic class: multi-output `cook` requires a block-form `using` clause. Exercises § 4.6 and App. A.4 multi-output rule.
```

> **Before committing any negative case:** run the Rust parser on its Cookfile and verify the substring in `error.txt` actually appears in the error message. If not, either (a) update `error.txt` to use a word that appears in the Rust parser's current diagnostic for that class, or (b) open a separate follow-up to improve the parser's wording — but do NOT change the class (what's being diagnosed). The goal is to pin the class, not the wording.

- [ ] **Step 6: Commit**

```bash
git add docs/standard/conformance/negative/
git commit -m "docs(standard): seed negative conformance corpus (5 cases)"
```

---

## Task 18 — Implement the `cook-lang` conformance harness (TDD)

**Files:**
- Modify: `cli/crates/cook-lang/Cargo.toml` (ensure `[[test]]` or integration-test dir is supported — default behavior is that files under `tests/` are integration tests; no change may be needed)
- Create: `cli/crates/cook-lang/tests/conformance.rs`

The harness is two integration tests: `positive_conformance_corpus` and `negative_conformance_corpus`. Each walks its corresponding subdirectory of `docs/standard/conformance/` and:

- **Positive:** parses the Cookfile, serializes the resulting AST with a stable pretty-printer into the `parse.txt` format, and asserts equality with the committed `parse.txt`.
- **Negative:** parses the Cookfile, asserts the result is `Err`, and asserts the error message contains the substring in `error.txt`.

Locating the corpus: integration tests run with the crate's directory as CWD. The corpus lives at `../../../docs/standard/conformance/` relative to `cli/crates/cook-lang/`. The harness resolves this robustly via `env!("CARGO_MANIFEST_DIR")`.

- [ ] **Step 1: Write the failing test first — positive corpus case 001**

Create `cli/crates/cook-lang/tests/conformance.rs` with the content below. This establishes the harness and runs it against case 001. Additional cases are discovered automatically by the directory walk; if they exist, they're run.

```rust
//! Conformance corpus harness.
//!
//! Walks `docs/standard/conformance/` and asserts that `cook-lang` parses
//! positive cases into the expected AST summary and rejects negative cases
//! with a diagnostic containing the expected class-substring.
//!
//! See `docs/standard/00-introduction.mdx` § 0.7 for conformance requirements.

use std::fs;
use std::path::{Path, PathBuf};

use cook_lang::ast::*;
use cook_lang::parse;

fn corpus_root() -> PathBuf {
    // cli/crates/cook-lang/tests/conformance.rs  →  cli/crates/cook-lang
    // ../../../ puts us at the repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/standard/conformance")
        .canonicalize()
        .expect("conformance corpus root missing")
}

fn case_dirs(sub: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dir = corpus_root().join(sub);
    for entry in fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {}", dir.display(), e))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn repr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c    => out.push(c),
        }
    }
    out.push('"');
    out
}

fn repr_list(xs: &[String]) -> String {
    let inner: Vec<String> = xs.iter().map(|s| repr(s)).collect();
    format!("[{}]", inner.join(", "))
}

fn format_using(u: &Option<UsingClause>) -> String {
    match u {
        None => "None".to_string(),
        Some(UsingClause::Shell(s))       => format!("Shell({})", repr(s)),
        Some(UsingClause::LuaBlock(s))    => format!("LuaBlock({})", repr(s)),
        Some(UsingClause::ShellBlock(xs)) => format!("ShellBlock({})", repr_list(xs)),
    }
}

fn format_step(step: &Step) -> String {
    match step {
        Step::Shell { command, interactive, .. } => {
            format!("Shell interactive={} command={}", interactive, repr(command))
        }
        Step::Lua { code, .. } => format!("Lua code={}", repr(code)),
        Step::LuaBlock { code, .. } => format!("LuaBlock code={}", repr(code)),
        Step::Cook { step, .. } => {
            format!(
                "Cook outputs={} using={}",
                repr_list(&step.outputs),
                format_using(&step.using_clause),
            )
        }
        Step::Plate { step, .. } => format!("Plate command={}", repr(&step.command)),
        Step::Test { step, .. } => {
            let timeout = match step.timeout {
                None    => "None".to_string(),
                Some(n) => format!("Some({})", n),
            };
            format!(
                "Test command={} timeout={} should_fail={}",
                repr(&step.command),
                timeout,
                step.should_fail,
            )
        }
    }
}

fn format_use(u: &UseStatement) -> String {
    format!("UseStatement module_name={} line={}", repr(&u.module_name), u.line)
}

fn format_import(i: &ImportDecl) -> String {
    format!(
        "ImportDecl name={} path={} line={}",
        repr(&i.name),
        repr(&i.path),
        i.line,
    )
}

fn format_var(v: &(String, String)) -> String {
    format!("({}, {})", repr(&v.0), repr(&v.1))
}

fn format_config(cb: &ConfigBlock) -> String {
    let name = match &cb.name {
        None    => "None".to_string(),
        Some(n) => format!("Some({})", repr(n)),
    };
    format!("ConfigBlock name={} body={} line={}", name, repr(&cb.body), cb.line)
}

fn format_cookfile(c: &Cookfile) -> String {
    let mut out = String::new();
    out.push_str("Cookfile\n");

    let uses: Vec<String> = c.uses.iter().map(format_use).collect();
    out.push_str(&format!("  uses: [{}]\n", uses.join(", ")));

    let imports: Vec<String> = c.imports.iter().map(format_import).collect();
    out.push_str(&format!("  imports: [{}]\n", imports.join(", ")));

    let vars: Vec<String> = c.vars.iter().map(format_var).collect();
    out.push_str(&format!("  vars: [{}]\n", vars.join(", ")));

    let configs: Vec<String> = c.config_blocks.iter().map(format_config).collect();
    out.push_str(&format!("  config_blocks: [{}]\n", configs.join(", ")));

    out.push_str("  recipes:\n");
    for r in &c.recipes {
        out.push_str(&format!(
            "    Recipe name={} line={}\n",
            repr(&r.name),
            r.line,
        ));
        out.push_str(&format!("      deps: {}\n", repr_list(&r.deps)));
        out.push_str(&format!("      ingredients: {}\n", repr_list(&r.ingredients)));
        out.push_str(&format!("      excludes: {}\n", repr_list(&r.excludes)));
        out.push_str("      steps:\n");
        for s in &r.steps {
            out.push_str(&format!("        {}\n", format_step(s)));
        }
    }
    out
}

fn normalize(s: &str) -> String {
    // Trim trailing whitespace on each line; drop trailing blank lines.
    let mut lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

#[test]
fn positive_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();

    for case in case_dirs("positive") {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input_path = case.join("Cookfile");
        let expected_path = case.join("parse.txt");

        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));
        let expected = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e));

        match parse(&input) {
            Ok(ast) => {
                let actual = format_cookfile(&ast);
                if normalize(&actual) != normalize(&expected) {
                    failures.push(format!(
                        "case {}: AST shape mismatch.\n--- expected (parse.txt) ---\n{}\n--- actual ---\n{}\n",
                        name,
                        normalize(&expected),
                        normalize(&actual),
                    ));
                }
            }
            Err(e) => {
                failures.push(format!(
                    "case {}: expected parse success, got error: {}\n",
                    name, e
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "positive conformance failures:\n\n{}",
        failures.join("\n")
    );
}

#[test]
fn negative_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();

    for case in case_dirs("negative") {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input_path = case.join("Cookfile");
        let expected_path = case.join("error.txt");

        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));
        let expected_substring = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e))
            .trim()
            .to_string();

        match parse(&input) {
            Ok(_) => {
                failures.push(format!(
                    "case {}: expected parse error, got success\n",
                    name
                ));
            }
            Err(e) => {
                let msg = format!("{}", e);
                if !msg.contains(&expected_substring) {
                    failures.push(format!(
                        "case {}: error did not contain expected substring\n  expected substring: {:?}\n  actual message:     {:?}\n",
                        name, expected_substring, msg,
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "negative conformance failures:\n\n{}",
        failures.join("\n")
    );
}
```

- [ ] **Step 2: Run the tests and watch them fail / diagnose**

```bash
cd cli && cargo test --test conformance -- --nocapture
```

Expected initial outcome: most cases pass, but some may fail because `parse.txt` values for Lua/shell blocks were written by hand in Task 16 and the parser's captured string may differ in leading whitespace. Also the negative corpus's `error.txt` substrings may not match the exact wording in the Rust parser's current error messages.

- [ ] **Step 3: For each failing positive case, copy the `actual` output into its `parse.txt`**

For each failure reported as "AST shape mismatch", the test output includes `--- actual ---` with the ground-truth format. Paste that content (unchanged) into the case's `parse.txt`. This is normative: the Rust parser's actual behavior is the source of truth (per the plan's source-of-truth rule).

- [ ] **Step 4: For each failing negative case, update `error.txt` to contain a substring that actually appears in the parser's current error**

The test output will show the actual message. Pick a short, class-distinguishing substring (e.g., "ingredients", "unclosed", "before") and write just that word or short phrase to `error.txt`. Do NOT change the class — change only the substring that captures it.

- [ ] **Step 5: Re-run and confirm all tests pass**

```bash
cd cli && cargo test --test conformance
```

Expected outcome:

```
running 2 tests
test positive_conformance_corpus ... ok
test negative_conformance_corpus ... ok

test result: ok. 2 passed; 0 failed
```

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-lang/tests/conformance.rs docs/standard/conformance/
git commit -m "test(cook-lang): add conformance harness against docs/standard/conformance/"
```

---

## Task 19 — Write `CONTRIBUTING.md`

**Files:**
- Create: `CONTRIBUTING.md` (repo root)

- [ ] **Step 1: Write the file**

Create `CONTRIBUTING.md` with exactly this content:

```md
# Contributing to Cook

## The Cook Standard

The Cookfile language is defined by the Cook Standard in [`docs/standard/`](docs/standard/). The Standard is the authoritative reference for the language. The Rust parser in `cli/crates/cook-lang/` is the current reference implementation; it is the de-facto authority for any Cookfile construct whose Standard chapter is presently a `NORMATIVE-TODO` stub.

### Spec-first rule

Any change that affects Cookfile surface syntax, execution semantics, the Cook Lua API, or the module system MUST:

1. Update `docs/standard/` in the same commit that modifies the implementation.
2. Add one entry to `docs/standard/D-changes.mdx` with a new stable `CS-NNNN` ID, a one-line summary, the sections affected, and the commit reference.
3. If the grammar changes, update `docs/standard/A-grammar.mdx`.
4. If the change is observable from a Cookfile, add at least one case to `docs/standard/conformance/positive/` or `docs/standard/conformance/negative/`.

Non-trivial language changes SHOULD be designed at the Standard level first; the implementation follows.

### Local enforcement

The repo ships a portable `pre-commit` hook at `.githooks/pre-commit` that inspects the staged diff and warns when you've touched language-surface code without also touching `docs/standard/`. Install it once per clone:

```bash
git config core.hooksPath .githooks
```

The hook's goal is to make language impact visible at commit time. If you're making a non-language-affecting change (refactor, performance work, error-message rewording), set `COOK_STANDARD_BYPASS=1` for that commit.

### Language-surface paths (what the hook watches)

- `cli/crates/cook-lang/**` — the lexer, parser, and AST
- `cli/crates/cook-luagen/**` — codegen that materializes language constructs
- `cli/crates/cook-register/**` — Cook Lua API registration
- `tree-sitter-cook/grammar.js` — tree-sitter grammar (currently out of date; see `docs/standard/D-changes.mdx` CS-0002)
- `tree-sitter-cook/src/**` — tree-sitter externals

If you add a new crate that contributes to language surface, update both this list and the hook.

### Conformance

- `cli/crates/cook-lang/tests/conformance.rs` walks `docs/standard/conformance/` and asserts the Rust parser's behavior. Run it with `cargo test -p cook-lang --test conformance`.
- A tree-sitter harness against the same corpus is planned; see `D-changes.mdx` CS-0002.

### Running the normative-keyword lint

```bash
bash scripts/check-normative-keywords.sh
```

The lint flags lowercase `must`/`shall`/`should`/`may` occurrences in normative chapters. Review each hit: either promote to all-caps (if the clause is meant to be binding) or reword (if the clause is descriptive).
```

- [ ] **Step 2: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING.md with Cook Standard spec-first rule"
```

---

## Task 20 — Write `.githooks/pre-commit`

**Files:**
- Create: `.githooks/pre-commit` (mode 0755)

- [ ] **Step 1: Write the hook**

Create `.githooks/pre-commit` with the content below and make it executable:

```bash
#!/usr/bin/env bash
#
# Cook Standard pre-commit hook.
#
# Warns when a commit touches language-surface code without also updating
# docs/standard/. Install with:
#     git config core.hooksPath .githooks
#
# Bypass with:
#     COOK_STANDARD_BYPASS=1 git commit ...
#
# See CONTRIBUTING.md for the spec-first rule.

set -euo pipefail

if [ "${COOK_STANDARD_BYPASS:-0}" = "1" ]; then
  exit 0
fi

staged="$(git diff --cached --name-only --diff-filter=ACMRT)"

if [ -z "$staged" ]; then
  exit 0
fi

is_language_surface() {
  case "$1" in
    cli/crates/cook-lang/*)       return 0 ;;
    cli/crates/cook-luagen/*)     return 0 ;;
    cli/crates/cook-register/*)   return 0 ;;
    tree-sitter-cook/grammar.js)  return 0 ;;
    tree-sitter-cook/src/*)       return 0 ;;
    *)                            return 1 ;;
  esac
}

is_standard() {
  case "$1" in
    docs/standard/*) return 0 ;;
    *)               return 1 ;;
  esac
}

touches_language=0
touches_standard=0

while IFS= read -r path; do
  [ -z "$path" ] && continue
  if is_language_surface "$path"; then
    touches_language=1
  fi
  if is_standard "$path"; then
    touches_standard=1
  fi
done <<EOF
$staged
EOF

if [ "$touches_language" = "1" ] && [ "$touches_standard" = "0" ]; then
  cat >&2 <<'MSG'
error: this commit touches language-surface code but does not update docs/standard/.

The Cook Standard is the authoritative definition of the Cookfile language;
language-affecting changes require an update to docs/standard/ in the same
commit. See CONTRIBUTING.md.

Staged language-surface paths:
MSG
  while IFS= read -r path; do
    [ -z "$path" ] && continue
    if is_language_surface "$path"; then
      printf '  %s\n' "$path" >&2
    fi
  done <<EOF
$staged
EOF
  cat >&2 <<'MSG'

To bypass for a non-language-affecting change (refactor, performance work,
error-message rewording), set COOK_STANDARD_BYPASS=1 for this commit.
MSG
  exit 1
fi

exit 0
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x .githooks/pre-commit
```

- [ ] **Step 3: Verify: run the hook manually against an empty index**

With no files staged (or only non-language-surface files staged), the hook MUST exit 0:

```bash
git stash -u
./.githooks/pre-commit
echo "exit=$?"
```

Expected: `exit=0` with no output. Then restore with `git stash pop` if you had changes.

- [ ] **Step 4: Verify: simulate a language-surface-only stage**

```bash
# Make a harmless whitespace change to a cook-lang file and stage only that
echo "" >> cli/crates/cook-lang/src/lib.rs
git add cli/crates/cook-lang/src/lib.rs
./.githooks/pre-commit
echo "exit=$?"
# Expected: exit=1 and an error message listing cli/crates/cook-lang/src/lib.rs.
# Clean up:
git restore --staged cli/crates/cook-lang/src/lib.rs
git restore cli/crates/cook-lang/src/lib.rs
```

- [ ] **Step 5: Commit**

```bash
git add .githooks/pre-commit
git commit -m "build: add pre-commit hook enforcing Cook Standard spec-first rule"
```

---

## Task 21 — Write `scripts/check-normative-keywords.sh`

**Files:**
- Create: `scripts/check-normative-keywords.sh` (mode 0755)

- [ ] **Step 1: Write the lint**

Create `scripts/check-normative-keywords.sh`:

```bash
#!/usr/bin/env bash
#
# Cook Standard normative-keyword lint.
#
# Flags lowercase occurrences of must/shall/should/may as whole words inside
# normative chapters of the Cook Standard. RFC 2119 keywords MUST appear in
# all-caps when they carry normative weight (§ 1.1). This lint catches
# accidental de-normative-ization during edits.
#
# Each flagged line is a candidate: the reviewer either promotes the keyword
# to all-caps (if the clause should be binding) or rewords (if the clause is
# descriptive).

set -euo pipefail

NORMATIVE_GLOB='docs/standard/0[0-9]-*.mdx docs/standard/A-*.mdx'

hits=0
for f in $NORMATIVE_GLOB; do
  [ -f "$f" ] || continue
  # -E extended regex; \b word boundaries. Skip lines inside fenced code blocks
  # by excluding lines that start with a triple backtick or sit between them.
  # We approximate: strip fenced regions with awk, then grep.
  filtered="$(awk '
    BEGIN { in_fence = 0 }
    /^```/ { in_fence = !in_fence; next }
    { if (!in_fence) print NR ":" $0 }
  ' "$f")"

  matches="$(printf '%s\n' "$filtered" | grep -E '\b(must|shall|should|may)\b' || true)"

  if [ -n "$matches" ]; then
    echo "== $f =="
    printf '%s\n' "$matches"
    hits=$((hits + 1))
  fi
done

if [ "$hits" -gt 0 ]; then
  echo ""
  echo "check-normative-keywords: lowercase RFC 2119 keywords found in $hits file(s)."
  echo "Review each hit: promote to all-caps if the clause is binding, or"
  echo "reword to remove the keyword if the clause is descriptive."
  exit 1
fi

echo "check-normative-keywords: OK"
exit 0
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x scripts/check-normative-keywords.sh
```

- [ ] **Step 3: Run it**

```bash
bash scripts/check-normative-keywords.sh
```

Expected outcome: likely non-zero on first run, because § 0 and § 1 contain descriptive prose with words like "should", "may", etc. Review each hit:

- If the word IS meant to be normative, promote to all-caps in the chapter file.
- If the word is NOT meant to be normative, reword. Common patterns:
  - "may appear" → "appears" or "is permitted to appear"
  - "should be" → "is"
  - "must be" (descriptive) → "is" or "needs to be"

- [ ] **Step 4: Fix hits in `00-introduction.mdx` and `01-notation.mdx`**

Edit the two fully-written chapters to address each flagged occurrence. The intent is: after this step, the lint passes.

Expected hits in the current text:
- `00-introduction.mdx`: "may introduce dual-track versioning" (§ 0.5) — reword to "A future Cook 1.0 release introduces dual-track versioning" or promote to "MAY".
- `00-introduction.mdx`: "may evolve freely" (§ 0.6) — promote to "MAY".
- `01-notation.mdx`: contains "must" in meta-discussion about the convention itself (e.g., "Lowercase occurrences of 'must', ..." which is quoting, not using). These appear inside code blocks or as quoted words; the awk fence filter only covers ``` fenced blocks, not inline backticks — so you may need to tweak the lint or reword the meta-text.

One pragmatic option: in `01-notation.mdx`, change quoted words like `'must'` to `` `must` ``-styled inline code and extend the lint to also skip inline code. Alternatively, rewrite the sentence to avoid using the literal words.

- [ ] **Step 5: Re-run and confirm**

```bash
bash scripts/check-normative-keywords.sh
```

Expected: `check-normative-keywords: OK` with exit 0.

- [ ] **Step 6: Commit**

```bash
git add scripts/check-normative-keywords.sh docs/standard/00-introduction.mdx docs/standard/01-notation.mdx
git commit -m "build: add scripts/check-normative-keywords.sh and clean up existing hits"
```

---

## Task 22 — Update `CLAUDE.md` to point at the Standard

**Files:**
- Modify: `CLAUDE.md` (repo root)

- [ ] **Step 1: Add a new "Cook Standard" section**

Open `CLAUDE.md`. Append the following section before the final (or after "Module Structure Rules", whichever places it most logically — it should be high-visibility):

```md
## The Cook Standard

The Cookfile language is defined by the Cook Standard in `docs/standard/`. The Standard is the authoritative reference for the language. Any change touching Cookfile surface syntax, execution semantics, the Cook Lua API, or the module system MUST update `docs/standard/` in the same commit — see `CONTRIBUTING.md` for the full rule.

Install the spec-first pre-commit hook once per clone:

    git config core.hooksPath .githooks

Run the conformance harness with:

    cargo test -p cook-lang --test conformance
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude): point at docs/standard/ and the spec-first hook"
```

---

## Task 23 — Add banners and pointers to `docs/architecture/`

**Files:**
- Modify: `docs/architecture/README.md` — add a pointer line near the top.
- Modify: `docs/architecture/parser.md` — add a banner at the very top.

- [ ] **Step 1: Add the pointer to `docs/architecture/README.md`**

Open `docs/architecture/README.md`. Immediately after the `## What Cook Is` paragraph (around line 5–6 of that section), insert a pointer paragraph:

```md
> **Language definition.** This directory documents how the implementation works. For the definition of the Cookfile language itself — syntax, semantics, Cook Lua API, modules — see `docs/standard/`.
```

- [ ] **Step 2: Add the banner to `docs/architecture/parser.md`**

Open `docs/architecture/parser.md`. At the very top, immediately after the `# Parser: Lexer, AST, and Parsing Pipeline` heading, insert:

```md
> **This document describes the Rust parser implementation.** For the definition of the Cookfile language, see `docs/standard/` — in particular `02-lexical.mdx`, `03-syntactic-grammar.mdx`, `04-recipes.mdx`, and `A-grammar.mdx`.
>
> This document may lag behind the Standard on specific constructs; when in doubt, the Standard (and the Rust parser source) are authoritative.
```

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/README.md docs/architecture/parser.md
git commit -m "docs(architecture): point at docs/standard/ for language definition"
```

---

## Task 24 — Final verification

This task has no file changes — it confirms everything wired in prior tasks still works together.

- [ ] **Step 1: Run the full test suite**

```bash
cd cli && cargo test
```

Expected outcome: all existing tests pass, plus the two new conformance tests. No test is skipped or ignored.

- [ ] **Step 2: Run the keyword lint**

```bash
bash scripts/check-normative-keywords.sh
```

Expected: `check-normative-keywords: OK`.

- [ ] **Step 3: Verify the pre-commit hook is installed and functional**

```bash
git config core.hooksPath
# Expected output: .githooks
```

If not `.githooks`, run `git config core.hooksPath .githooks` to install. The hook will be active for subsequent commits in this clone.

- [ ] **Step 4: Walk the directory tree and confirm structure**

```bash
find docs/standard -type f | sort
```

Expected files (one per line):

```
docs/standard/00-introduction.mdx
docs/standard/01-notation.mdx
docs/standard/02-lexical.mdx
docs/standard/03-syntactic-grammar.mdx
docs/standard/04-recipes.mdx
docs/standard/05-execution-model.mdx
docs/standard/06-cook-lua-api.mdx
docs/standard/07-modules.mdx
docs/standard/A-grammar.mdx
docs/standard/B-rationale.mdx
docs/standard/C-examples.mdx
docs/standard/D-changes.mdx
docs/standard/README.mdx
docs/standard/conformance/negative/001-unterminated-string/Cookfile
docs/standard/conformance/negative/001-unterminated-string/error.txt
docs/standard/conformance/negative/001-unterminated-string/notes.md
docs/standard/conformance/negative/002-bare-at-prefix/Cookfile
docs/standard/conformance/negative/002-bare-at-prefix/error.txt
docs/standard/conformance/negative/002-bare-at-prefix/notes.md
docs/standard/conformance/negative/003-use-after-recipe/Cookfile
docs/standard/conformance/negative/003-use-after-recipe/error.txt
docs/standard/conformance/negative/003-use-after-recipe/notes.md
docs/standard/conformance/negative/004-duplicate-ingredients/Cookfile
docs/standard/conformance/negative/004-duplicate-ingredients/error.txt
docs/standard/conformance/negative/004-duplicate-ingredients/notes.md
docs/standard/conformance/negative/005-multi-output-using-string/Cookfile
docs/standard/conformance/negative/005-multi-output-using-string/error.txt
docs/standard/conformance/negative/005-multi-output-using-string/notes.md
docs/standard/conformance/positive/001-empty-recipe/Cookfile
docs/standard/conformance/positive/001-empty-recipe/notes.md
docs/standard/conformance/positive/001-empty-recipe/parse.txt
... (through 010-use-and-module-call/)
```

Confirm no `.gitkeep` remains.

- [ ] **Step 5: Review the commit log**

```bash
git log --oneline -30
```

Expect roughly one commit per task (24 commits in sequence) plus the final verification.

- [ ] **Step 6: No final commit needed**

Task 24 is verification-only; nothing to commit. The skeleton PR is complete.

---

## Notes for the executor

- **Source of truth.** Wherever a step says "run the Rust parser and compare" or "copy the actual output", the Rust parser is the authority. Do NOT reach for `tree-sitter-cook/grammar.js` to resolve ambiguity at any point in this plan. Tree-sitter conformance is the follow-up plan's job (CS-0002).
- **Committing per task.** Each task ends with a commit. Commits are scoped to their task; do not bundle. If a task's verification surfaces an issue that requires editing a prior task's file (e.g., `parse.txt` needs updating because the Rust parser's capture differs from what was hand-written in Task 16), fix the file in place and include it in the current task's commit.
- **Hook not self-blocking.** The pre-commit hook you write in Task 20 will not block any commits in this plan because each commit either (a) touches no language-surface code or (b) touches `docs/standard/` simultaneously. If for some reason the hook fires unexpectedly during plan execution, set `COOK_STANDARD_BYPASS=1` for that commit and note the reason.
- **Don't write `taste`.** Parser.md still mentions a `taste` step; the current AST (`Step` enum in `ast.rs`) has no `Taste` variant. The `taste` token has been removed. Don't add it to the Standard.
