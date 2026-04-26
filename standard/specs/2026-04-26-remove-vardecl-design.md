# Design: Remove `VarDecl` in favor of config-block-only variables

**Date:** 2026-04-26
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Scope:** Cook Standard only. CLI/parser and tree-sitter follow-ups are out of scope for this design; they will conform to the Standard version that ships with this change (semver tagging tracked under a separate, follow-up CS).

## 1. Motivation

The top-level `variable_declaration` form `NAME "value"` (lexed as `Token::VarDecl`) is the only Cookfile mechanism for setting a name-value pair outside a `config` block. It is being removed for three reasons:

1. **It does not compose with named configs.** A `CC "gcc"` written outside any `config` block cannot be overridden by a named config without ad-hoc precedence rules; folding the surface into config blocks gives a single, layered mechanism with well-defined override semantics.
2. **It duplicates a more general surface.** A `config` block body is raw Lua and can perform the same `cook.env.X = "value"` write directly. The bare top-level form is sugar for one specific case of what config blocks already express.
3. **It carries spec cost disproportionate to its value.** `VarDecl` requires a contextual blocking-keyword reservation (§{lexical.keywords}), a lexical line-classification rule that is shared across top-level and recipe-body positions and then reclassified inside recipes (§{grammar.step-dispatch}), and rationale prose explaining the asymmetry. Removing the form deletes all of that.

## 2. Non-goals

- **Implementation tracking.** The CLI and tree-sitter projects will reference the Standard version they conform to; this design adds no implementation checklist to the Standard.
- **Cookfile-side migration tooling.** Pre-1.0 lockstep posture (per `project_cook_standard.md`); existing Cookfiles that use the top-level form will fail to parse after this change. No deprecation window.
- **`env` alias scope expansion beyond config blocks.** Module bodies and other Lua-bearing scopes currently use `cook.env.X`; widening the alias is a separate question.
- **Multi-select overlays.** The composition rule below permits at most one named overlay; relaxing this to multi-select is a future, non-breaking change if real use cases arise.
- **Standard versioning / conformance-claim mechanism.** Tracked as a follow-up CS that introduces semver tagging on the Standard and amends `D-changes.mdx` to group entries under tags.

## 3. Design

### 3.1. Lexical (§2)

- Remove the `VarDecl` row from the §2.1 token table.
- Remove the §{lexical.line-classification} test-11 rule that produces `Token::VarDecl`. After this change, a line of the shape `BARE_IDENTIFIER STRING` at the top level produces a `Content` token, which is rejected as not a valid `toplevel_item` (§{grammar.overview}). Inside a recipe body it remains a `Content` token and is dispatched as a `shell_command` by the unchanged step-dispatch cascade (§{grammar.step-dispatch} priority 7).
- Remove the "blocking keywords" reservation in §{lexical.keywords}. With `variable_declaration` gone, no contextual reservation against keyword left-hand sides is required. The reserved-recipe-segment set (`stem`, `name`, `ext`, `dir`, `in`, `out`, `all`) is unaffected.
- Update Note 2.1.1 / Note 2.4.1 / Note 2.9.1 and any other in-place references to `VarDecl` or to `test_var_after_recipe_is_shell_command`. Where a note references an external test file, replace the reference with an in-spec example or remove it (consistent with the user's standing direction in `standard/notes.txt` to make the spec self-contained).

### 3.2. Syntactic grammar (§3) and Appendix A

- Remove §3.3 ("Variable declarations") in its entirety, including Examples 3.3.1 and Note 3.3.1.
- Remove `variable_declaration` from the `toplevel_item` alternation in §3.1 prose and in App. A.1. Remove the `variable_declaration ::= ...` production from App. A.2 and the "Identifiers blocked from `variable_declaration`" paragraph that follows it.
- Update §3.2 ("Top-level ordering"): drop `variable_declaration` from the four-form list; the surviving ordered forms are `use_declaration`, `import_declaration`, `config_block`. Rewrite Example 3.2.1 to set the equivalent value via a `config` block:

  ```cook
  use cpp
  import backend ./services/backend

  config
      env.CC = "gcc"
  end

  config release
      env.CXXFLAGS = "-O3"
  end

  recipe build
      gcc -o main main.c
  end
  ```

  Update Note 3.2.1 to drop the variable-declaration ordering test reference.
- Update §3.6 ("Config blocks") to add a brief lead-in pointing to the new §3.6.1 subsection on composition.
- Update §3.8 ("Step dispatch inside a recipe"): remove the paragraph that reclassifies `Token::VarDecl` to a `shell_command` inside a recipe body. With the token gone, the step-dispatch cascade handles the case directly via priority 7. Update the §3.8 example accordingly.
- Update App. A.4 `shell_command` grammar comment: drop the parenthetical "A line of the form NAME 'value' (which would be a variable_declaration at the top level) is treated as a shell_command inside a recipe body." reference. After removal, no special wording is needed — the line is just a `Content` line dispatched by priority 7.

### 3.3. New normative content: config-block composition

A new normative passage is inserted as a subsection of §3.6 ("Config blocks"), with slug `grammar.config-composition`. Placement as a subsection avoids any renumber of the existing §3.7 ("Recipes", `grammar.recipe-syntax`) and §3.8 ("Step dispatch", `grammar.step-dispatch`); their slugs are preserved either way, but keeping the section numbers stable keeps inbound diff churn minimal.

> **§3.6.1. Config-block composition [#grammar.config-composition].** A Cookfile MAY contain at most one unnamed `config_block` (the *base config*) and zero or more named `config_block`s (the *overlay configs*). At load time, a conforming implementation MUST select at most one overlay config by name. The selection mechanism is implementation-defined.
>
> A conforming implementation MUST report a load-time error when the selected name does not match any named `config_block` declared in the Cookfile. The diagnostic MUST identify the requested name.
>
> **Execution order.** When a base config is present, its body MUST execute first against the Cook Lua API state (§{lua}). When an overlay is selected, its body MUST execute second against the same state. The overlay's writes therefore observe values established by the base; an overlay write to a key already set by the base replaces the base's value (last-write-wins). When no overlay is selected, only the base (if present) executes. When neither is present, no `config_block` Lua executes during the load phase.
>
> **Phase.** Both bodies execute during the load phase, after `use` resolution (§{lua.use-env}) and before recipe registration (§{exec}).

Add §3.6.1.1 (or unnumbered "Example") showing base + two overlays with last-write-wins:

```cook
use cpp

config
    env.CC = "gcc"
    env.CXXFLAGS = "-O0 -g"
end

config release
    env.CXXFLAGS = "-O3"
end

config dev
    env.CXXFLAGS = "-O0 -g -DDEBUG"
end

recipe build
    cook "build/main" using "{CC} {CXXFLAGS} -o {out} main.c"
end
```

When no overlay is selected, `{CXXFLAGS}` resolves to `-O0 -g`. When `release` is selected, the base runs first (`env.CXXFLAGS = "-O0 -g"`), then `release` runs (`env.CXXFLAGS = "-O3"`); `{CXXFLAGS}` resolves to `-O3`.

### 3.4. §6. Cook Lua API: `env` alias

Add a short normative paragraph in §6 (location: end of §6.1, or as a new §6.1.x — exact placement decided during writeup).

> Within the body of a `config_block` (§{grammar.config-blocks}), the bare global `env` MUST be bound such that `env` and `cook.env` refer to the same table. Writes through either name are observable through both. The `env` alias is in scope only within `config_block` bodies; it is not bound in recipe-body Lua, in `using` blocks, in module bodies, or at any other Lua entry point.

This brings the §3.2.1 / §3.6.1 / §3.7.1 examples (which use `env.X = ...`) into normative conformance.

### 3.5. Cross-recipe references (§5) and placeholders (§6.7)

The placeholder fallback `{TOKEN} → cook.env[TOKEN]` (§{xref.resolution} step 4, §{lua.shell-placeholders}) is **unchanged in behaviour and unchanged in text**. Both spec sites already reference `cook.env[TOKEN]` directly without invoking the VarDecl form. No edit required.

B.6.4 (`rationale.recipe-name-priority`), which justifies recipe-name precedence over `cook.env` in placeholder resolution, is unaffected — it does not reference VarDecl or the top-level variable form. No edit required.

### 3.6. Rationale (Appendix B) updates

- **Rewrite B.2.4 (`rationale.contextual-keywords`).** The current paragraph justifies a contextual reservation that covers two cases: the LHS of a `variable_declaration` and the final segment of a recipe name. After this change only the recipe-segment half remains. Rewrite the subsection to motivate the contextual reservation purely in terms of the reserved recipe-segment set; drop the variable-declaration half and the parenthetical "(where ambiguity with keyword-prefixed step forms would otherwise arise)". The closing paragraph about §{lexical.keywords} carrying "two lists instead of one" is also removed — only one list survives.
- **Delete B.3.8 (`rationale.name-value-shell`)** in its entirety. The subsection rationalises a special reclassification of the top-level `NAME "value"` form when it appears inside a recipe body. With the top-level form gone, no reclassification is happening — the line is a `Content` line dispatched by step-priority 7, identical to any other unrecognised content. (Verified: no cross-references to this slug exist outside the subsection's own definition; deletion is safe.)
- **Add a new B.3.x subsection: "Config blocks as the sole variable surface."** Placement: after the surviving B.3 subsections, before §B.4. Captures:
  - The composition motivation (named overlays cannot override values set outside any config block without ad-hoc precedence).
  - The single-mechanism win (one syntax, one execution model, one phase).
  - Why layered single-select was chosen over multi-select for this revision (avoids ordering questions across multiple selected overlays; non-breaking to relax later).
  - Why the selection mechanism is implementation-defined (config selection is a CLI-tool affordance; the Standard's general posture is to specify behavior, not invocation surfaces).

### 3.7. D-changes (Appendix D) and conformance

- Add a single D-changes entry (CS-NNNN) summarising:
  - Removal of `VarDecl` token, `variable_declaration` production, and the blocking-keyword reservation.
  - New §3.6.1 normative composition rules.
  - New `env` alias paragraph in §6.
  - Rationale and example sweeps.
- `standard/conformance/`: any test that depends on a top-level `NAME "value"` parsing as `VarDecl` is removed or rewritten. Any test that exercises `var-after-recipe-is-shell-command` survives but is rephrased — it is no longer a "reclassification" test, just a "this line is a shell command" test.
- New conformance tests added for §3.6.1:
  - **base-only:** unnamed config sets `env.X`; `{X}` resolves to that value when no overlay is selected.
  - **base + overlay last-write-wins:** unnamed config sets `env.X = "a"`; named overlay sets `env.X = "b"`; selecting the overlay yields `{X} = "b"`.
  - **overlay-only:** no unnamed config; named overlay sets `env.X`; selecting it yields the value; not selecting it yields the placeholder fallback (warning per existing rules).
  - **missing-overlay-is-error:** selecting a name that does not match any `config_block` produces a load-time error whose diagnostic names the requested overlay.
  - **base-runs-before-overlay:** ordering observable through Lua side effects (e.g., a counter incremented in both bodies).

## 4. Out-of-scope items surfaced during brainstorm

These are explicitly *not* addressed by this design and should be tracked separately:

- Standard semver tagging and a `D-changes.mdx` reshape grouping entries by version (next CS).
- Adjacent items in `standard/notes.txt` (quoted recipe names, dropping `recipe` keyword, dropping `end`, the `all` reserved segment, declaration-only cook steps, etc.) — each is its own brainstorm.
- Whether `env` should also alias `cook.env` inside module bodies — deferred until module-body authoring patterns warrant it.

## 5. Review checklist for the implementation plan

When `writing-plans` produces the implementation plan from this design, the plan SHOULD include at least:

- One step per spec file touched (`02-lexical.mdx`, `03-syntactic-grammar.mdx`, `04-recipes.mdx`, `06-cook-lua-api.mdx`, `appendix/A-grammar.mdx`, `appendix/B-rationale.mdx`, `appendix/D-changes.mdx`, plus any cross-reference targets).
- A verification pass that no `§{...}` reference is broken (the existing `rehype-bare-ref-lint` pipeline catches this; the plan should run it). With the §3.6.1-subsection placement chosen above no renumber is required, but the lint still confirms slug stability.
- One step per new conformance test under `standard/conformance/`.
- A final pre-commit step that runs the conformance harness (`cargo test -p cook-lang --test conformance`) and confirms the spec-first hook passes.
