# Design: Test-step language additions for the v1.0 test runner (`as` modifier, caching contract, `add_test` field defaults)

**Date:** 2026-05-07
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Scope:** Cook Standard (chapters §4.8, §6.4, §8.6, App. A.4, App. B.4, App. D), the Rust parser (`cli/crates/cook-lang`), the codegen (`cli/crates/cook-luagen`), `cook-register`'s `cook.add_test` API, `tree-sitter-cook`, and conformance fixtures.

> **Companion spec.** The runner architecture, CLI surface, cache backend layout, JSON/JUnit sidecar shapes, and reporter UX live in `docs/superpowers/specs/2026-05-07-test-runner-design.md`. That document is implementation-only and does not change the Cook Standard. The two specs land together in one PR per the Standard-governs-language rule.

## 1. Motivation

CS-0046 (2026-05-03) deleted the v0.7-track `cook --test` runner. The commit message named the reasons: half-shipped sub-flags that printed "not yet implemented" and ran everything anyway, a `test_output` scaffolding that was never wired to engine results, and a runner that needed CS-0041 to surface non-zero exits at all. Shipping that into v1.0 would have frozen a broken contract. CS-0046's last sentence — *"Test execution is implementation-defined as of v0.8"* — created a deliberate gap to be filled before 1.0.

The companion runner-architecture spec fills the gap on the implementation side. This Standard change fills the gap on the language side. Three normative additions are required for the runner to land with a stable contract:

1. **A way to name a test for the report.** The runner produces a per-test summary; today the only name a `test { … }` shell step carries is the auto-generated `<recipe>:test#N[<iteration-item>]` shape. That shape is fine as a default but inadequate when authors want a human-readable name. The Lua surface already has `cook.add_test({ name = … })`; the shell surface lacks an analog.
2. **A normative caching contract for test units.** CS-0024 §2 explicitly punted on this — *"the Standard says implementations MAY cache and stops there."* That posture worked while the runner was being redesigned; for v1.0 the Standard has to commit. Cook's value proposition includes test-run caching as a first-class feature, and the cache key has to be specified normatively so that a passing test on machine A is a cache hit on machine B (the cloud-cache scenario, CS-0054 et seq.).
3. **`cook.add_test` field defaults nailed down.** Today's `test_api.rs` defaults the `suite` field to the empty string and `command` to the empty string when omitted. The `command` default is wrong (a test with no command is a no-op disguised as a pass); the `suite` default is incoherent with the report grouping the runner needs to do (group by suite ⇒ every empty-suite test gets pooled together regardless of recipe).

A fourth, smaller contract-fix piggybacks: `should_fail` interaction with caching. Today's implementation never caches tests; the runner caches passing tests; passing-with-`should_fail` (the body exited non-zero, which is what the author wanted) MUST cache as a pass. That rule needs to be normative because cross-machine cache replay only works if every conforming implementation interprets the same recorded outcome the same way.

This CS makes those four additions. It does not introduce a `volatile` per-step cache-bust modifier; the operator-side `cook --test --rerun [PATTERN]` covers the residual case (see §B-new.4 for rationale). It does not introduce a `test recipe` or `recipe-modifier` form; CS-0024's "test_step is a cook step with no declared output" model is preserved.

## 2. Non-goals

- **A new step keyword or recipe modifier.** Plate and test stay distinct; recipes that contain `test_step`s remain ordinary recipes. The runner's identification of "what is a test" is the presence of `test_step` units, not a recipe-level flag.
- **A `volatile` modifier or other per-step cache-bust attribute.** The CLI handles this via `--rerun [PATTERN]`. Adding a per-step attribute would teach authors that hidden inputs are normal (B-new.4); the right answer for tests with hidden inputs is to declare them via the existing `discovered_inputs` mechanism (CS-0052-era).
- **A schema for the JSON or JUnit test report.** Sidecar shapes are a runner-implementation concern, not a Standard concern. They live in the companion spec.
- **CLI flag specification.** The Standard does not specify CLI arguments; the runner spec does.
- **Implementation of the test-result cache backend.** The Standard specifies the cache *contract* (what's hashed, what's recorded, when results MUST NOT cache); the implementation lives in `cook-cache`'s test-result keyspace per the companion spec.
- **Multi-language test-framework integration.** Cook does not parse vitest/pytest/playwright output. A test_step that shells out to such a runner is a single Cook test from the Standard's perspective; sub-test granularity is the framework's affair.

## 3. Design

### 3.1. The `as STRING` modifier on `test_step`

Grammar (App. A.4 update):

```ebnf
test_step      ::= "test" body test_modifiers NEWLINE
test_modifiers ::= ("as" STRING)? ("timeout" NUMBER)? "should_fail"?
```

`body` is the production CS-0024 already defined: `shell_block | using_lua_block`. The `as` clause is a new optional leading modifier.

The `STRING` argument is the test's **display name**. Without it, the test's display name is the implementation's auto-generated identity (typically `test#N` for the Nth `test_step` in a recipe, with an iteration discriminator appended for non-one-shot modes). With it, the auto-generated identity is replaced — except for the iteration discriminator, which still appends in iteration modes so each unit has a unique identity.

The `STRING` admits the same placeholder substitution as the body: `test { … } as '$<in.stem>-roundtrip'` substitutes per CS-0033 (`$<IDENT>` sigil placeholder syntax) at register-time codegen, producing one display name per iteration. When the substituted name varies per iteration, the iteration-discriminator suffix is omitted (the name itself is already discriminating).

The display name is metadata — it is **not** part of the test's cache fingerprint (§3.3). Renaming a test does not bust its cache.

#### 3.1.1. Modifier order

Modifier order is **fixed**: `as → timeout → should_fail`. A conforming implementation MUST reject any out-of-order modifier sequence. The diagnostic MUST name the offending modifier and the canonical position. Examples:

```cook
# ACCEPTED — canonical order
test { foo $<in> } as 'name' timeout 30 should_fail
test { foo $<in> } as 'name' timeout 30
test { foo $<in> } as 'name' should_fail
test { foo $<in> } timeout 30 should_fail
test { foo $<in> } timeout 30
test { foo $<in> } should_fail
test { foo $<in> } as 'name'
test { foo $<in> }

# REJECTED — `as` after `timeout`
test { foo $<in> } timeout 30 as 'name'
# Diagnostic: "modifier `as` must precede `timeout` in test_step (canonical
#   order: as → timeout → should_fail) at line N"

# REJECTED — `should_fail` before `timeout`
test { foo $<in> } should_fail timeout 30
# Diagnostic: "modifier `timeout` must precede `should_fail` in test_step at line N"
```

The fixed order keeps the grammar LL(1) and reads consistently with §A.4's existing modifier-rule posture (which already treats `timeout` before `should_fail`).

#### 3.1.2. Interaction with Lua `cook.add_test({ name = … })`

A Lua-block test MAY use both surfaces:

```cook
test >{
    cook.add_test {
        command = './' .. input,
        name = 'inner-name',
    }
} as 'outer-name' timeout 30
```

When both are present, the modifier value (`'outer-name'`) wins. Rationale: modifiers are declarative on the step; the `add_test` table is one expression in the body; the Standard chooses the declarative source as authoritative when they conflict. A diagnostic MAY be emitted for the conflict, but is not normative — both are syntactically valid.

#### 3.1.3. `as` on non-test steps is rejected

`as STRING` is a `test_step`-only modifier in this CS. A conforming implementation MUST reject `as` on `cook_step` or `plate_step`. Rationale: the modifier's only role is naming a test for the report; cook and plate steps have no analogous report position.

A future CS may extend `as` to other step kinds (e.g., to label cache slots in the build viewer); that extension is out of scope here.

### 3.2. `cook.add_test` table — fields nailed down

§6.4's `cook.add_test` specification is updated to enumerate every field, its type, its default, and its semantics:

| Field | Type | Default | Notes |
|---|---|---|---|
| `command` | string | — (required) | Shell command (the body, post-substitution). Empty string is a load-time error. |
| `name` | string | auto-generated | Display name. When omitted, the implementation generates `test#N[<iteration-item>]` per §3.1. |
| `suite` | string | enclosing recipe's name | When omitted, MUST default to the enclosing recipe's name (for namespaced recipes, the fully-qualified name including the namespace). |
| `timeout` | number | 300 | Seconds. Implementations MAY clamp to a minimum of 1; non-positive values MUST be rejected. |
| `should_fail` | bool | false | Inverts pass/fail interpretation per §3.4. |

A conforming implementation MUST:
- Reject a `cook.add_test` call where `command` is the empty string (or omitted) at register time. Diagnostic: `cook.add_test: command field is required and must be a non-empty string at <recipe>:<line>`.
- Default `suite` to the enclosing recipe's fully-qualified name when omitted, **not** the empty string. (This is the contract-fix from §1.)
- Reject `timeout ≤ 0` with diagnostic `cook.add_test: timeout must be a positive number, got <value>`.

Unknown field keys SHOULD be flagged with a warning but MUST NOT be a hard error (forward-compatibility: a future CS may add fields).

### 3.3. Caching contract for test units

A new normative subsection in §8.6, immediately after §8.6's existing cache-decision rules for cook units. Verbatim text intent (final wording in the Standard PR):

> §8.6.x Test-unit caching.
>
> Conforming implementations MAY cache the result of a `test_step` unit. When caching is implemented, the following rules MUST hold.
>
> 1. **Cache key inputs.** A test unit's cache key MUST be a content-addressed digest derived from at least the following inputs:
>    - The unit's substituted command text. For shell-block tests this is the command string after CS-0033 sigil-placeholder substitution; for Lua-block tests this is the Lua source body string with bindings resolved per §6.4.
>    - The content fingerprints of every cook-step output the test consumes via §3.3 source-list flattening (CS-0024).
>    - The content fingerprints of every recipe-output the test references via `cook.dep_output(NAME)` or `$<NAME>` cross-recipe substitution (§5.5).
>    - The deterministic env-key set as filtered through `cook-fingerprint`'s `EnvDenylist` (the existing fingerprint mechanism for cook units).
>    - The tool-binary hash set per CS-0052.
>    - The `timeout` modifier value.
>    - The `should_fail` modifier value.
>
>    The cache key MUST NOT include the test's display name (§3.1), the `suite` field (§3.2), the recipe name, or the step's source position. Renaming a test, moving it within a recipe, or moving a recipe between Cookfiles MUST NOT bust its cache.
>
> 2. **Recordable outcomes.** A conforming implementation MAY record a test result for replay only when the unit's outcome is **passed**. A test is **passed** when:
>    - The body's exit status is success and `should_fail` is false; or
>    - The body's exit status is failure and `should_fail` is true.
>    A test that is **failed** (the dual of passed), **timed out**, or **blocked** (its build dependency failed) MUST NOT be cached. The implementation MAY discard a previously-cached entry whose corresponding test is observed to fail in a subsequent run.
>
> 3. **Replayed payload.** A cache hit MUST produce, for reporting purposes, the originally-recorded `(stdout, stderr, duration_secs, success)` quadruple. The duration value of a cache replay is the recorded original duration, not the wall-clock cost of the cache lookup.
>
> 4. **Cross-machine portability.** A test result recorded on machine A MUST be a cache hit for the same fingerprint on machine B, given identical inputs (see §1's input list and the cloud-cache portability rules of §8.6.<existing-cross-machine-subsection>).

Implementations MAY provide operator-side cache-bypass (the runner spec describes `cook --test --rerun [PATTERN]`). The Standard does not require any specific operator surface; it requires only that the caching behavior, when present, match this contract.

### 3.4. Pass/fail interpretation in the presence of `should_fail`

A clarification of existing §4.8 wording, made unambiguous for the cache contract:

> A test unit's outcome is determined by combining the body's exit status with the `should_fail` modifier:
>
> | Body exit | `should_fail` | Outcome |
> |---|---|---|
> | success (0) | false | passed |
> | success (0) | true | failed (`should_fail` was claimed; body did not fail) |
> | failure (≠0) | false | failed |
> | failure (≠0) | true | passed |
> | timed out | (any) | timed out (a distinct outcome; not passed even if `should_fail` was true) |

The "timed out + `should_fail`" row is the subtle one and is normatively pinned: a timeout is not a controllable failure mode, so claiming `should_fail` and observing a timeout is an unsatisfied claim.

### 3.5. Phase classification

§8.1.2's table gains no new rows — `as STRING` is purely a register-phase substitution, which is already the phase for `test { … }` and `test >{ … }` per CS-0024's table.

The `as` substitution runs in the same `expand_template_to_lua_with_deps` pass that handles body substitution; the resulting display name is a literal string baked into the `cook.add_test({ name = … })` call the codegen emits.

### 3.6. App. A grammar deltas

```ebnf
# Updated:
test_modifiers ::= ("as" STRING)? ("timeout" NUMBER)? "should_fail"?
```

The `test_step` production itself is unchanged from CS-0024 (still `"test" body test_modifiers NEWLINE`); only `test_modifiers` is extended.

The "Step-dispatch priority" list in §A.4 is unchanged.

### 3.7. Tree-sitter parser deltas

`tree-sitter-cook`'s `grammar.js`:

- `test_step`'s modifier slot is extended to admit an optional leading `as_modifier` field: `field("as_name", optional(seq("as", $.string)))`.
- Highlights query (`queries/highlights.scm`): add `(test_step as_name: (string) @string.special)` so the displayed name highlights distinctly from the body text.
- Corpus fixtures parallel the conformance suite per §3.10.

### 3.8. App. B (informative annex) — rationale subsections to add

Three new B.4 subsections, in addition to the existing CS-0024 B subsections:

#### B-new.1 — Why `as STRING` and not `name STRING`

`as` reads as natural English ("this test, as 'parses-config'") and aligns with SQL's `AS` aliasing posture. `name` would collide with the table-field `name` in `cook.add_test`, creating two ways to spell the same concept with subtly different timing (modifier substitutes at register time; `name` field substitutes at execute time when the table is constructed). Choosing distinct surface tokens for the two surfaces makes the timing difference legible.

#### B-new.2 — Why the modifier order is fixed

LL(1) parsability is a hard constraint for the Cook parser (`cook-lang`'s recursive-descent shape). Free-order modifiers either require backtracking or a peek/look-ahead that the parser doesn't currently support. The fixed order also produces canonical reading order in source: identity (name) → constraint (timeout) → interpretation (should_fail). Authors who reach for a different order are flagged immediately rather than silently parsed differently.

#### B-new.3 — Why test caching is universal in the Standard

The alternative was caching as an implementation choice with no Standard contract. That posture works for v0.x but creates a portability hazard at v1.0: a Cookfile that passes on cloud-cache machine A and fails on machine B (because A cached a test, B did not) is a contract violation only if the Standard says so. Specifying the cache key inputs and the recordable-outcomes rule means cross-machine cache replay is correct or it is a Standard violation. Without a contract, the cloud-cache feature degrades to "works on machines configured the same way," which is the wrong promise for a build-system test runner.

#### B-new.4 — Why no per-step `volatile` modifier

A `volatile` modifier teaches authors that hidden inputs are normal. The right answer for tests with filesystem inputs Cook cannot see (vitest reading `.test.ts`, pytest reading `.py`) is the existing `discovered_inputs` mechanism (CS-0052-era) plus depfile emission. The right answer for tests with non-filesystem hidden inputs (network, clock, $RANDOM) is the operator-side `cook --test --rerun [PATTERN]` flag, applied per-invocation. A persistent per-step opt-out would convert "I know this is non-deterministic" into "I'm too lazy to declare my inputs," and the cache becomes a sieve.

The CLI escape hatch costs nothing in language surface and pushes authors toward correct input declaration; the language escape hatch costs spec breadth and grants permission for sloppiness. The trade is in favor of the CLI form.

#### B-new.5 — Why timed-out + `should_fail` is "timed out", not "passed"

A timeout is not an exit status; it is a runtime intervention. Treating it as failure-equivalent for the purpose of `should_fail` would mean any sufficiently slow test passes any `should_fail` claim, which destroys the modifier's diagnostic value. The dedicated "timed out" outcome preserves the runner's ability to flag timeouts distinctly in the report.

### 3.9. App. D (D-changes) — entry to add

```
CS-NNNN  2026-05-07  test-step language additions for v1.0 test runner
                     New `as STRING` modifier; cache contract for test units;
                     `cook.add_test` field defaults nailed; `should_fail` ×
                     timeout interaction pinned. Companion runner-impl spec
                     in cli/. No surface deprecations.
```

### 3.10. Conformance fixtures

Under `standard/conformance/`:

**positive/**
- `test_as_modifier_parses.cook` — every modifier-order subset accepted.
- `test_as_with_substitution.cook` — `as '$<in.stem>-roundtrip'` produces per-iteration name.
- `test_add_test_default_suite.cook` — `cook.add_test({ command = '…' })` with no `suite` field gets the recipe's fully-qualified name as suite.
- `test_should_fail_pass_table.cook` — every row of §3.4's table with a fixture that exercises it.
- `test_cache_key_independence.cook` — two recipes with byte-identical test bodies, identical inputs, different recipe names; verifies they share a cache slot (renaming doesn't bust).

**negative/**
- `test_as_after_timeout.cook` — `test { foo } timeout 30 as 'name'` rejected with B.4.modifier-order diagnostic.
- `test_as_after_should_fail.cook` — `test { foo } should_fail as 'name'` rejected.
- `test_should_fail_before_timeout.cook` — `test { foo } should_fail timeout 30` rejected.
- `test_as_non_string.cook` — `test { foo } as 5` rejected.
- `test_as_on_cook_step.cook` — `cook "out" using { … } as 'foo'` rejected with diagnostic naming `as` as test-step-only.
- `test_as_on_plate_step.cook` — `plate { … } as 'foo'` rejected with the same diagnostic.
- `test_add_test_empty_command.cook` — `cook.add_test({ command = '' })` rejected.
- `test_add_test_negative_timeout.cook` — `cook.add_test({ command = 'x', timeout = -1 })` rejected.

## 4. Migration

CS-0046 already removed the prior `cook --test` runner *and* deleted its three pinning fixtures (`cli_audit_test_exit_code/`, `cmd_test_inferred_deps_e10/`, `cross_cookfile_test/`). There is no user-facing v0.x→v1.0 transition: nobody is running `cook --test` today; nobody is relying on the `as STRING` modifier syntax (it does not exist); nobody depends on the empty-string suite default (the runner that consumed it is gone).

The `recipe FOO` invocation path that runs `test_step`s as side effects is **unchanged**. Existing Cookfiles using `test { … }` work identically. Adding `as STRING` is a forward-compatible surface addition; defaulting `suite` to the recipe name (rather than empty string) is a behavior change visible only via `cook.add_test` table inspection or the (now non-existent) report grouping — no existing test contract relies on an empty string here.

Touched surfaces:

- `standard/src/content/docs/04-recipes.mdx` — §4.8's modifier table extended per §3.1.
- `standard/src/content/docs/06-cook-lua-api.mdx` — §6.4's `cook.add_test` field table extended per §3.2.
- `standard/src/content/docs/08-execution-model.mdx` — §8.6 gains the test-cache subsection per §3.3; §4.8 gains the §3.4 outcome table.
- `standard/src/content/docs/appendix/A-grammar.mdx` — `test_modifiers` production updated per §3.6.
- `standard/src/content/docs/appendix/B-rationale.mdx` — five new B.4 subsections per §3.8.
- `standard/src/content/docs/appendix/D-changes.mdx` — CS-NNNN entry per §3.9.
- `standard/conformance/positive/`, `negative/` — fixtures per §3.10.
- `examples/test_benchmarks/Cookfile` — every `test_step` shape exercised; this becomes the runner-impl conformance pin (companion spec).
- `examples/monorepo_test/`, `examples/test_caching/` — additional runner-impl fixtures (companion spec).
- `tree-sitter-cook/grammar.js` and `tree-sitter-cook/test/corpus/` — per §3.7.
- `Cookfile` (top-level) and any in-repo `Cookfile`s — no rewrite needed; existing `test { … }` syntax is unchanged.
- `cook_modules/*.lua` — no rewrite needed; `cook.add_test` calls without `suite` continue to work (now with a more useful default).

## 5. Implementation impact

### 5.1. `cli/crates/cook-lang`

- `ast::TestStep` gains an `as_name: Option<String>` field (parallel to `timeout: Option<u64>` and `should_fail: bool`).
- `recipe.rs`'s test-modifier parsing path admits a leading `as STRING` token; emits a parse error with the canonical-order diagnostic on out-of-order modifiers.
- `tests.rs` gains coverage for §3.10's positive and negative fixtures.

### 5.2. `cli/crates/cook-luagen`

- `test_step.rs::generate_test_step` propagates `as_name` into the emitted `cook.add_test({ name = … })` table when present. The substitution path is the existing `expand_template_to_lua_with_deps` — the `as` STRING goes through the same per-iteration substitution as the body.
- When `as_name` is present and contains an iteration-driving placeholder (`$<in>`, `$<in.X>`), the codegen uses the substituted result as the literal `name` for that iteration's emitted unit (no discriminator suffix). When `as_name` is present and constant, the codegen appends a discriminator (`[$<in.stem>]`) for iteration modes; that's runner-side metadata, not codegen.

### 5.3. `cli/crates/cook-register`

- `test_api.rs::register_test_api`'s `add_test` body changes its `suite` default from `""` to the enclosing recipe's fully-qualified name. The fully-qualified name is available in `CaptureState` (today's `current_recipe` field).
- `add_test` body validates `command` is non-empty and `timeout > 0` per §3.2.

### 5.4. `cli/crates/cook-engine` and `cook-fingerprint`

The cache-contract impact lives in the runner-impl spec; on the language-spec side, the only change is that fingerprint inputs MUST include `timeout` and `should_fail` (§3.3 inputs 6 and 7). `cook-fingerprint` extends its hasher accordingly. This is a small additive change.

### 5.5. `tree-sitter-cook`

Per §3.7. Corpus updates land alongside the grammar.js changes.

### 5.6. Conformance fixtures

Per §3.10. The conformance harness at `standard/conformance/` regenerates against the new AST.

## 6. Open questions

None blocking. Two design choices the spec makes explicitly that the implementation plan should call out:

1. **`suite` default = recipe name, not `""`.** This is a behavior change visible to any tool that introspects `cook.add_test` payloads. No such tool exists in tree (the runner that consumed `suite` was removed); the change lands cleanly.

2. **`as` is test-step-only in this CS.** A future CS may extend `as` to `cook_step` and `plate_step` for build-viewer labeling. That extension is intentionally deferred; this CS does not pre-allocate the surface.

## 7. Acceptance criteria

The Standard PR for CS-NNNN is acceptance-complete when:

1. §4.8, §6.4, §8.6, App. A.4, App. B.4, and App. D are updated as described.
2. The conformance fixtures listed in §3.10 exist and pass.
3. The reference implementation (`cook-lang`, `cook-luagen`, `cook-register`, `cook-fingerprint`) and `tree-sitter-cook` parse, codegen, and highlight the new surface; the `cargo test --workspace` and `cargo test -p cook-lang --test conformance` suites are green.
4. Every Cookfile in `examples/`, `cook_modules/`, the top-level repo, and the `tree-sitter-cook/` subproject continues to parse identically (the additions are surface-additive only; no existing surface changes shape).
5. The Standard's "Recent Changes" appendix (App. D) gets an entry naming this CS-NNNN.
6. The companion runner-impl spec (`docs/superpowers/specs/2026-05-07-test-runner-design.md`) is on the same PR and references this CS.
