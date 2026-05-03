# Design: `$<NAME>` sigil-disambiguated placeholder syntax with closed-set resolution

**Date:** 2026-05-03
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time; expected CS-0033)
**Linear epic:** TBD (Cook pre-1.0 hardening)
**Predecessors:**
- App. E.2 (`${...}` syntax in `using` strings) — original framing of shell/template collision, partially addressed by CS-0022/CS-0024
- App. E.8 (chore-body shell commands skip `{TOKEN}` substitution) — symptom of the same root cause from a different angle
- CS-0011 (top-level `NAME "value"` removal) — precedent for a hard surface-syntax cut
- CS-0022 (cook-step iteration unification) — established the closed placeholder set for `cook` bodies
**Scope:** `standard/src/content/docs/02-lexical.mdx`, `standard/src/content/docs/05-cross-recipe-references.mdx`, `standard/src/content/docs/06-cook-lua-api.mdx`, `standard/src/content/docs/appendix/A-grammar.mdx`, `standard/src/content/docs/appendix/D-changes.mdx`, `standard/src/content/docs/appendix/E-pre-v1-checklist.mdx`, `standard/conformance/**`, `cli/crates/cook-lang` (lexer scanner only), `cli/crates/cook-luagen` (template scanner + substitution path), `cli/crates/cook-register`, `examples/**`, all in-tree `Cookfile`s.

## 1. Motivation

Cook's placeholder syntax — `{NAME}`, `{NAME.ACCESSOR}`, `{in}`/`{out}`/`{all}`, etc. — is a headline feature: it lets authors write build steps that read like the shell command they always wanted, with build-system data interpolated in line. The syntax is concise, visually quiet, and composes cleanly across cook-step output patterns, cook-step `using` bodies, plate/test bodies, and bare shell commands in recipe and chore bodies.

The syntax also collides with shell. Both Cook and POSIX shell use `{` and `}` as significant characters. Today's collision surface, verified empirically against `cli/crates/cook-luagen/src/template.rs` and runtime behavior:

| Shell construct | Today's behavior in `using { … }` |
|---|---|
| `{a,b,c}` (brace expansion) | substituted as `cook.env["a,b,c"]` |
| `{1..3}` (range expansion) | substituted as `cook.env["1..3"]` |
| `${HOME:-fallback}` (parameter expansion with default) | scanner sees inner `{HOME:-fallback}`, substitutes as `cook.env["HOME:-fallback"]`; emits `echo $` followed by the lookup result |
| `'{"key":"value"}'` (JSON literal) | substituted as `cook.env["\"key\":\"value\""]`, recorded into `consulted_env_keys` (cache poisoning) |
| `awk '{print $1}'` | substituted as `cook.env["print $1"]` |
| `find . -exec rm {} \;` | substituted as empty-token `cook.env[""]` lookup |

Every legitimate shell idiom involving `{}` is silently broken. App. E.2 originally claimed CS-0022 resolved this by passing block contents "verbatim to the shell"; that claim is incorrect — the lexer's brace-balancing finds the *end* of the block, but the substitution scanner still walks the interior and consumes every `{...}` as a placeholder candidate.

A separate, contradictory bug in the same layer: bare shell commands inside recipe and chore bodies skip placeholder substitution entirely at codegen time. The literal `{ADB} devices` reaches the shell unchanged and fails with `/bin/sh: {ADB}: command not found`. App. E.8 framed this as a chore-only issue; it actually applies to all bare shell commands in any recipe or chore body.

The two bugs are complementary symptoms of one design defect: **the substitution layer cannot tell what is a Cook placeholder from what is shell content.** It is too aggressive in `using { … }` blocks (eats real shell) and absent in bare shell commands (passes through Cook placeholders unsubstituted). Implementations that try to "fix" one half worsen the other.

This design replaces the `{NAME}` placeholder shape with a sigil — `$<NAME>` — that has zero collision with POSIX shell. The change makes the substitution layer's job structurally trivial: a Cook placeholder is exactly `$<IDENT>` (with a strict identifier shape and a closing `>`), and nothing else in shell text needs special handling. The substitution scanner's bytes-per-decision drops from "scan every `{` and inspect the inner" to "scan every `$<` and verify the strict pattern."

The change also closes the env-var typo class. Today's §xref.resolution step 4 falls through to `cook.env[TOKEN]` for any unrecognized token, returning the empty string for a missing key. A typo like `{HOEM}` becomes empty silently. This design tightens the contract: every `$<TOKEN>` MUST resolve to a builtin, an in-scope recipe, or an env var declared in a config block. Otherwise, codegen fails with a diagnostic.

## 2. Non-goals

- **Lua-body syntax change.** Lua syntax owns its own braces (table literals, function bodies, etc.). Lua bodies — `> ...`, `>{ ... }`, `>>{ ... }`, and `using >{ ... }` — already do not participate in textual placeholder substitution; they receive iteration data and recipe access via the existing `cook.dep_output()`, `cook.env[...]`, and `using_lua_block` bindings (§{lua.using-block-globals}). Nothing in this design changes Lua bodies.
- **Ingredient glob syntax.** `ingredients "src/**/*.rs"` strings are glob patterns, not templates. They contain no placeholders today and gain none here.
- **Dep-list syntax.** `recipe verify: prepare` continues to take bare names; no sigil involved.
- **`use`/`import` path syntax.** These take strings and bare identifiers; no placeholder layer.
- **The `STRING` literal escape problem (App. E.3).** Recipe names, ingredient paths, and output patterns still use the unescaped STRING shape. The output-pattern *placeholder* layer changes here; the underlying STRING lexer does not. App. E.3 stays open.
- **Backwards-compatible dual grammar.** Pre-1.0, this is a flag-day cut. Both grammars accepted simultaneously is rejected as more complex than the migration it would smooth (§6).
- **Cache-key compatibility.** Cookfiles migrated to the new syntax produce different `command` strings, which produce different cache keys. This is correct — the underlying invocations are unchanged, but the recorded source differs. Pre-1.0 Cookfiles migrate together with their cache state (i.e., a clean rebuild on first run after migration).

## 3. Architecture

### 3.1. Placeholder lexical shape

A **Cook placeholder** in shell text is exactly the byte sequence:

```
$ < IDENT >
```

where:

- `$` is the literal byte `0x24`.
- `<` is the literal byte `0x3C`.
- `>` is the literal byte `0x3E`.
- `IDENT` is one of the following productions:
  - `bare_ident       := ALPHA (ALPHA | DIGIT | "_" | ".")*`
  - `out_indexed      := "out_" DIGIT+`
  - `out_indexed_acc  := "out_" DIGIT+ "." ACC`
  - `ACC              := "stem" | "name" | "ext" | "dir"`
  - `ALPHA            := "a"…"z" | "A"…"Z" | "_"`
  - `DIGIT            := "0"…"9"`

There are no internal whitespace, no escape sequences inside `IDENT`, no nesting.

The scanner finds a `$` byte, looks at the next byte. If it is not `<`, the `$` is literal shell. If it is `<`, the scanner attempts to consume `IDENT` followed by `>`. If `IDENT` does not match the production, or if `>` is not the immediately-following byte, the entire `$<...` sequence is literal shell — the scanner does not search forward for a `>`. This is by design: a malformed `$<...` looks identical to a literal `$<...` to the substitution layer, removing the "did the author mean a placeholder or shell?" ambiguity from the lexer.

A consequence: an author can write the literal text `$<HOME>` in shell by breaking the strict pattern. `$\<HOME>` works (shell collapses `\<` to `<`); `$<HOME${EMPTY}>` works (the inner stops being identifier-shape after `H`). No formal escape mechanism is added.

### 3.2. Closed-set resolution

For a successfully-lexed placeholder `$<TOKEN>`, codegen resolves TOKEN by trying these rules in order; first match wins:

1. **Builtin.** TOKEN matches one of:
   - `in`, `in.ACC` — current iteration item (one-to-one mode only)
   - `out`, `out.ACC` — single declared output (single-output step only)
   - `out_N`, `out_N.ACC` — N-th declared output (multi-output step only)
   - `all` — input list, space-joined (many-to-one mode only)

   Builtins are validated against the step's iteration mode and output declaration count exactly as today's §6.7 requires. Mode/count violations are diagnostic-emitting load-time errors with the same wording.

2. **In-scope recipe.** TOKEN is the name of a recipe reachable from the current Cookfile — own recipes, recipes imported via `use`, and recipes reachable through sigil-imported Cookfiles per §7. The match uses today's §xref.resolution scope rules; only the lookup key shape changes.

   `TOKEN.ACC` is a recipe accessor. The recipe must have a single declared output; the accessor applies to that output's path via `path.ACC`. Multi-output recipes accessed via accessor are rejected with the same diagnostic shape used today for `{NAME.ACC}` against multi-output recipes.

3. **Declared env var.** TOKEN appears as `env.TOKEN = ...` (or its `cook.env.TOKEN = ...` Lua equivalent) in a config block reachable from the current Cookfile's config-block scope. Resolution lowers to `cook.env["TOKEN"]` and the key is recorded in the unit's `consulted_env_keys` for cache invalidation purposes.

4. **Hard error at codegen.** A diagnostic enumerates: declared env vars in scope, recipes in scope, builtins valid in the current step's mode. The diagnostic suggests `env.TOKEN = os.getenv("TOKEN")` (or `cook.env.TOKEN = os.getenv("TOKEN")` in a config block) if the author meant to pass through an OS env var.

This is a **closed function**: every well-lexed placeholder either resolves to a known thing or fails loudly. Today's silent-empty-string fallthrough is gone.

#### 3.2.1. The `env.` namespace prefix

Step 2 (in-scope recipe) and step 3 (declared env var) can in principle conflict if a Cookfile declares `env.foo` and also has a recipe named `foo`. To remove ambiguity:

- Bare `$<foo>` resolves per the order above — recipe wins over env var. A Cookfile that declares both gets a load-time warning (not error), suggesting the author rename one or use the explicit form.
- Explicit `$<env.foo>` always resolves to `cook.env["foo"]` regardless of recipe name. This form is reserved at the lexer level: a TOKEN beginning with `env.` is *always* an env-var lookup, never a recipe accessor.

The `env` namespace is reserved at the recipe-name level too, in two ways:

- A recipe segment equal to `env` (under today's `RESERVED_RECIPE_SEGMENTS` mechanism) is rejected. The existing rule already prevents `recipe env` and `recipe foo.env`; CS-0033 adds `env` to that list.
- A recipe whose **first** dotted segment is `env` (e.g., `recipe env.foo`) is also rejected at parse time. Without this restriction, `$<env.foo>` would have two valid readings (env-var lookup of `foo`, or accessor on recipe `env.foo`); the rule eliminates the ambiguity at the source rather than at resolution.

### 3.3. Surface coverage

The new syntax applies to every position that today admits a `{TOKEN}` placeholder:

| Position | Example before | Example after |
|---|---|---|
| Cook-step output pattern | `cook "build/{in.stem}.o"` | `cook "build/$<in.stem>.o"` |
| Cook-step shell-block body | `using { gcc -c {in} -o {out} }` | `using { gcc -c $<in> -o $<out> }` |
| Plate shell-block body | `plate { cp {in} dest/ }` | `plate { cp $<in> dest/ }` |
| Test shell-block body | `test { diff {in} expected }` | `test { diff $<in> expected }` |
| Bare shell command (recipe body) | `@{ADB} devices` | `@$<ADB> devices` |
| Bare shell command (chore body) | `{ADB} devices` | `$<ADB> devices` |
| Cross-recipe body ref | `cat {lib.proto_lib} > out` | `cat $<lib.proto_lib> > out` |

Lua-body forms — including `using >{ ... }` — are **not** affected. Their access shapes (`cook.dep_output(...)`, `cook.env[...]`, `using_lua_block` bindings) are syntactic Lua and need no placeholder layer.

The parser change is local: every position that today calls into the placeholder scanner now expects the new shape. The parser's structure, recipe model, step-kind dispatch, and cross-Cookfile resolution are unchanged.

### 3.4. Substitution applies uniformly

Today's split between "substitution at codegen for `using` blocks" and "no substitution for bare shell commands" is removed. Every shell text — bare shell command in any body, cook-step `using { ... }`, plate body, test body — runs through the same substitution pass at codegen. With the new strict scanner, the pass is cheap: most shell text contains no `$<` at all and walks bytewise to the next character.

This closes App. E.8 as a side effect of the syntax change. The chore-body codegen path no longer needs a special placeholder pass; all paths share one.

### 3.5. Cache observables

Three improvements fall out:

1. **`consulted_env_keys` becomes accurate.** Today's poisoning by garbage entries (`"\"key\":\"value\""`, `"print $1"`) disappears. Every entry is a real, declared env var TOKEN.
2. **The cache key for a recipe whose body contains literal `{...}` shell now reflects the actual command bytes.** Today, the literal shell uses get scrambled into env lookups whose results depend on env state, making the cache key falsely env-sensitive. After: literal shell bytes pass through unchanged into `command`, and the hash is over what actually runs.
3. **Diagnostics catch typos at codegen, not at cache-replay.** `$<HOEM>` fails at register-time with a clear "no env var HOEM declared" message, instead of silently producing an empty value that propagates through cache and re-runs.

## 4. Spec changes (normative)

### 4.1. §{lexical} additions

Add a new subsection: **§2.X. Placeholders in shell text.** Defines the strict `$<IDENT>` lexical shape per §3.1. Locates this in the lexical chapter rather than §6.7 because the shape is a property of the lexer that fires across multiple grammatical contexts.

Update the existing reserved-segment list to include `env` (per §3.2.1).

### 4.2. §{xref.resolution} rewrite

Replace step 4 ("fall through to `cook.env[TOKEN]`") with the closed-set rule per §3.2. The chapter-level statement that a missing env var produces the empty string is replaced with: "every placeholder MUST resolve to a builtin, in-scope recipe, or declared env var; failure is a diagnostic-emitting load-time error."

Add §xref.declared-env-vars defining what "declared" means: an `env.NAME = expr` assignment in a config block, evaluated under the same config-block scope rules that already apply.

### 4.3. §{lua.shell-placeholders} rewrite

§6.7's placeholder table loses its "{TOKEN} (none of the above) → cook.env[TOKEN]" row from §6.7.1. Every other row keeps its semantics with the syntax updated to `$<...>`. The "Phase" preamble adds: "Substitution is performed by the code generator at register time over the strict `$<IDENT>` lexical shape defined in §{lexical.placeholders}."

§6.7.1 (plate/test placeholders) gets the same treatment.

### 4.4. App. A grammar updates

Add a `placeholder` production:

```
placeholder       = "$<" placeholder_ident ">"
placeholder_ident = bare_ident | out_indexed | out_indexed_acc
bare_ident        = ALPHA (ALPHA | DIGIT | "_" | ".")*
out_indexed       = "out_" DIGIT+
out_indexed_acc   = "out_" DIGIT+ "." accessor
accessor          = "stem" | "name" | "ext" | "dir"
```

Reference the placeholder production from `output_pattern`, `shell_block_body`, `bare_shell_command`, and `plate_test_shell_block_body`.

### 4.5. App. D — CS-0033

New entry under v0.7. Cross-references CS-0022 (where the closed set was originally defined for cook bodies) and CS-0024 (which extended block bodies to plate/test). Records the migration and the precise spec sections changed.

### 4.6. App. E updates

- **E.2** moves to fully resolved. Today's "Resolved in v0.5" status under-claims; the resolution under CS-0033 is structural, not partial. Update wording.
- **E.3** stays open with no change in scope. The STRING-escape problem is orthogonal.
- **E.8** moves to resolved as a CS-0033 side effect. Note that the resolution applies uniformly (recipe + chore + plate + test bodies), not chore-only.

## 5. Implementation notes (informative)

### 5.1. Single substitution pass

Replace the existing two-path codegen (one path for `using`/`plate`/`test` blocks via `template.rs`, another path that bypasses substitution for bare shell commands) with a single pass that walks shell text byte-by-byte, finds `$<`, and applies the strict-pattern matcher. The matcher returns one of three outcomes:

- `Match(Builtin | Recipe(..) | Env(..))` — emit a Lua-side concatenation against the resolved value
- `Match(UnknownToken)` — emit a diagnostic with the resolution failure message
- `NoMatch` — bytes pass through unchanged (the `$<...` was not a placeholder shape)

The existing `extract_brace_tokens` and `expand_with_deps_fallback` functions in `cli/crates/cook-luagen/src/{dep_ref.rs,template.rs}` are replaced wholesale.

### 5.2. `cook standard.migrate-to-sigil` chore

Ship a one-shot migration tool as a chore in `standard/Cookfile`. The tool:

1. Parses every `**/Cookfile` under the workspace.
2. For each placeholder position the parser identifies (output patterns, shell-block bodies, bare shell commands, plate/test bodies), rewrites `{TOKEN}` → `$<TOKEN>` in the source bytes.
3. Writes the result back. Comments and unrelated whitespace are preserved (the rewrite only touches placeholder spans).

The tool is parser-aware: it does not text-substitute `{...}` blindly. A `{` inside a Lua body, a glob pattern, or comment text is not touched. The conformance corpus is the test bed for the tool — every fixture's `Cookfile` rewrites cleanly, and the fixtures' `parse.txt` regenerate against the same parser.

### 5.3. Conformance corpus

Mass rewrite of all positive and negative fixtures under `standard/conformance/`. The rewrite runs as part of the CS-0033 commit and is not a backwards-compatibility break (pre-1.0 corpora are tied to their tag per App. E.1's documented operating rule).

New fixtures added:

- **Positive:** legitimate shell `{}` idioms inside `using`, `plate`, `test`, recipe-body bare shell, and chore-body bare shell. Covers brace expansion (`{a,b,c}`), range expansion (`{1..3}`), parameter expansion (`${HOME:-fallback}`), JSON literal (`'{"key":"value"}'`), awk script (`awk '{print $1}'`), and find-exec (`find . -exec rm {} \;`).
- **Negative:** undeclared `$<UNKNOWN>` produces a hard error with the diagnostic shape pinned. Reserved-namespace `recipe env` rejected at parse time. Bare placeholder `$<>` (empty IDENT) is literal shell, not an error.

### 5.4. Tree-sitter conformance

Tree-sitter-cook is presently at v0.4 + CS-0022 (App. E.4). CS-0033 widens the gap to v0.7. The CS-0002 follow-up that owns the tree-sitter audit picks up CS-0033 as part of its catch-up scope; no separate work item.

### 5.5. Examples and docs

Every `examples/**/Cookfile` rewrites via the migration tool. Every `.mdx` page under `standard/src/content/docs/` that contains a code block with `{TOKEN}` placeholder syntax updates to `$<TOKEN>`. The `B-rationale.mdx` chapter gains a new section explaining why the sigil syntax replaced the brace syntax (collision with shell, closed-set resolution, diagnostic improvements).

## 6. Migration

Pre-1.0, this is a flag-day cut. The case for dual-grammar transition (accept both `{NAME}` and `$<NAME>` for one version, then drop `{NAME}`) was considered and rejected:

- **Author confusion.** Two valid spellings invite "is this old or new?" reading friction. The tooling exists to convert in seconds.
- **Lexer complexity.** A dual scanner needs to disambiguate at the lexer layer — exactly the surface area this design exists to simplify.
- **Documentation churn.** Every doc and example would have to choose a spelling, then update again at the deprecation cut. Two churn rounds vs. one.

The migration sequence:

1. Land CS-0033 as a single commit on a `feature/sigil-placeholders` branch. The commit includes: Standard updates, parser/luagen changes, mass rewrite of conformance fixtures and examples, the `migrate-to-sigil` tool, tree-sitter version-banner bump (deferring grammar rewrite to CS-0002 follow-up).
2. Tag `cs-standard/v0.7` on the merge commit.
3. Publish a one-page migration note: install the new Cook, run `cook standard.migrate-to-sigil` in your workspace, commit the diff. Done.

External Cookfiles (downstream users) follow the same path. Pre-1.0, no API stability is owed; the tool makes the migration mechanical.

## 7. Open questions

- **Diagnostic wording.** The "no such env var X declared" diagnostic should suggest the right pass-through pattern. Current candidates: `env.X = os.getenv("X")` in a config block, vs. `env.X = cook.env.X or ""`, vs. a new `env.X = passthrough()` helper. To be settled in the implementation plan with author-experience review.
- **Reserved-namespace policy.** §3.2.1 reserves `env` as a TOKEN prefix. Should `out_`, `in_`, `all_` also be reserved to leave room for future builtins? Conservative answer: yes, but only as a recipe-name prefix restriction, not a TOKEN prefix restriction. Worth a paragraph in the spec but not a blocker.
- **`out_N` upper bound.** The spec doesn't bound `N` for `out_N`. A misformed `$<out_99>` against a 2-output recipe is currently a load-time diagnostic per §6.7; that stays. No new bound needed.
- **Migration of in-the-wild downstream Cookfiles.** Beyond this repo's `examples/`, downstream projects (the user's `glovemap`, etc.) need the migration tool to be runnable against arbitrary trees. The tool is a chore in `standard/Cookfile`; it should also be packaged as a standalone subcommand (`cook migrate sigil`) for downstream use. Decision deferred to the implementation plan.
