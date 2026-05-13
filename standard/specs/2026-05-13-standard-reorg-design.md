# Cook Standard structural redesign — design

**Date:** 2026-05-13
**Status:** Approved, pre-implementation
**Target version:** Cook Standard v0.10
**Author:** shiny_guru
**Track:** Standard editorial (no normative behaviour changes for conforming Cookfiles)

## 1. Context

The Cook Standard at v0.9 is organised as a parser walkthrough: lexical → syntactic grammar → recipes → cross-recipe references → Lua API → cross-Cookfile composition → execution model → standard modules. That ordering reflects the order a parser implementation reads source, not the order an author or implementor needs to consult the document.

Concrete symptoms of the current shape:

- Chapter 4 (Recipes and step kinds) is ~590 lines covering six distinct topics: recipe header, body termination, dependency list, ingredients, all step kinds, body bundling, and a removed module-call step kept for migration. Body bundling in particular is execute-phase behaviour that lives inside a Cookfile-surface chapter.
- Chapter 4a (Chores) is numbered `4a` with sidebar order `4.5` — an "in-between" status that signals an unresolved structural decision.
- The two-phase execution model is introduced in §6 (Lua API), restated in §8.1 (Execution model), and the normative phase-classification table sits at §8.1.2. Readers asking "what runs when" must read three chapters.
- Step dispatch is defined in §3.9, restated in §4.4, and again in Appendix A.4 — three near-duplicates that must stay synchronised.
- Sigil placeholders are split across §2.11 (token), §5.2 (resolution), §6.7 (cook-step tables), and §6.7.1 (plate/test tables) — four places.
- Cache concerns are split across §6.2.1 (`discovered_inputs` field syntax), §7.7 (portability invariants), and §8.6 (semantics, integrity, test-unit caching).
- The `use` declaration lives in §6.8 (Lua API) despite being a top-level Cookfile construct. §7 (Cross-Cookfile composition) explicitly tells the reader the cut is wrong: "the `use` declaration is defined in §6, not in this chapter."
- §9 (Standard Modules) holds 313 lines for one blessed module (`cc`); the catalogue index and the per-module specification are not separated, and adding the next blessed module bloats the same file.
- Appendix D (Changes) is ~1193 lines of chronological CS-NNNN entries with no per-version subheaders.
- Appendix E (Pre-1.0 checklist) follows Appendix D (Changes), inverting the natural order: forward-looking work after backward-looking history.

This document specifies a structural redesign of the Cook Standard that addresses these symptoms by reorganising the document into four explicit Parts with focused per-topic chapters, modelled on the ISO C specification convention already endorsed for this project.

## 2. Goals and non-goals

### Goals

1. Each chapter has one job; cross-cutting topics (phase classification, cache, placeholders) have a single normative home.
2. The Cookfile language (Part I) is fully readable by an author who never opens the Lua API or execution model material.
3. Chapter slugs convey topic, not parser-implementation phase ordering.
4. The blessed module catalogue scales additively: each blessed module lives in its own file.
5. The transition preserves every existing anchored URL via build-emitted redirects and every internal cross-reference via a slug-rename registry.
6. The reorg surfaces the small amount of new normative prose that the new shape requires (~160 lines covering Cook Lua API surfaces previously specified only in CS-0066/0070/0071 amendments) and lands it in the same cut.

### Non-goals

1. Any change to the Cookfile language surface, syntax, or semantics. A conforming Cookfile at v0.9 is byte-equivalent-conformant at v0.10.
2. Any change to the conformance corpus or to pinned CS-NNNN entries.
3. Editorial cleanup that does not block the reorg. Examples-quality, internal-file references in Notes, "stand on its own" hardening, and `App.` linking hygiene are tracked as separate workstreams.
4. Tree-sitter parity (CS-0002 follow-up).
5. Resolution of App. E.1 (backwards-conformance harness coupling).

## 3. Chapter map

The Standard reorganises into front matter (3 chapters), four Parts, and six annexes.

### 3.1. Front matter

| § | Title | Slug | Source today |
|---|---|---|---|
| 0 | Introduction | `intro` | current §0 (purpose, scope, non-scope, normative/informative, versioning) |
| 1 | Conformance | `conf` | extracted from current §0.7 |
| 2 | Notation and conventions | `notation` | current §1 |

### 3.2. Part I — The Cookfile language

| § | Title | Slug | Source today |
|---|---|---|---|
| 3 | Lexical structure | `lexical` | current §2.1–§2.10 (placeholders move out) |
| 4 | Top-level structure | `toplevel` | current §3.1, §3.2, §3.7.5, and §4.1.1 (body termination is a general structural rule) |
| 5 | Declarations | `decl` | current §3.4–§3.7 (`use`, `import`, `config`, `register`) |
| 6 | Recipes | `recipes` | current §3.8, §4.1, §4.2, plus the recipe-body region rule promoted from Note 4.4.2 |
| 7 | Chores | `chores` | current §4a (promoted to a real chapter) |
| 8 | Step kinds | `steps` | current §3.9, §4.3, §4.4, §4.5–§4.10 (one normative home for step dispatch and every step body) |
| 9 | Placeholders | `phl` | current §2.11, §6.7, §6.7.1 (consolidated) |
| 10 | Cross-recipe references | `xref` | current §5 (resolution stays here; cook-step placeholder tables move to §9) |
| 11 | Cross-Cookfile composition | `comp` | current §7.1–§7.5 (workspace root and cache invariants move to Part II) |
| 12 | Modules | `mods` | current §6.8 (`use`), the module lifecycle, and §9.1 catalogue index |

### 3.3. Part II — Execution model

| § | Title | Slug | Source today |
|---|---|---|---|
| 13 | Two-phase model | `exec.phases` | current §8.1, §8.1.2 (phase classification) |
| 14 | Capture mode | `exec.capture` | current §8.2 |
| 15 | Step groups and parallelism | `exec.groups` | current §8.3 plus body bundling moved from §4.9.3 |
| 16 | Cross-recipe ordering and interactive drain | `exec.ord` | current §8.4, §8.4.1, §8.5 |
| 17 | Cache semantics | `exec.cache` | current §8.6, §8.6.1, §8.6.3, §8.6.4, plus §7.7 portability invariants |
| 18 | Output materialisation | `exec.mat` | current §8.7 |
| 19 | Diagnostic ordering | `exec.diag` | current §8.8 |
| 20 | Workspace root | `exec.ws` | current §7.6 |

### 3.4. Part III — The Cook Lua API

| § | Title | Slug | Source today |
|---|---|---|---|
| 21 | Surface overview | `lua` | current §6.1, §6.1.1, §6.1.2 |
| 22 | Register-phase API | `lua.reg` | `cook.add_unit`, `cook.exec`, `cook.interactive`, `cook.recipe`, `cook.add_test`, `cook.step_group` |
| 23 | Execute-phase API | `lua.exe` | using-block globals, plate/test Lua bindings |
| 24 | Both-phase API | `lua.both` | `cook.sh`, `cook.load_module`, plus new normative definitions for `cook.env`, `cook.cache`, `cook.export`, `cook.import`, `cook.platform`, `cook.dep_output`, `cook.dep_output_list` |
| 25 | `fs.*` filesystem helpers | `lua.fs` | current §6.5 including the project-root sandbox (§6.5.8) and shell escape hatches (§6.5.9) |
| 26 | `path.*` path helpers | `lua.path` | current §6.6 |

### 3.5. Part IV — Standard module catalogue

| § | Title | Slug | Source today |
|---|---|---|---|
| 27 | Catalogue governance | `cat` | current §9.1 (bootstrap, vendoring, catalogue index) |
| 28 | `cc` — C-family build module | `cat.cc` | current §9.2 (own file: `28-cc.mdx`) |
| 28+ | Future blessed modules | `cat.<name>` | one `.mdx` per blessed module |

### 3.6. Annexes

| § | Title | Slug | Source today |
|---|---|---|---|
| A | Grammar (normative) | `grammar-appendix` | current Appendix A |
| B | Worked examples | `examples` | current Appendix C |
| C | Rationale | `rationale` | current Appendix B (renumbered headings; slugs preserved) |
| D | Pre-1.0 checklist | `pre-v1` | current Appendix E (moved before Changes) |
| E | Changes | `changes` | current Appendix D (per-version subheaders added) |
| F | Conformance corpus | `corpus` | new stub for the future "embed corpus" workstream |

## 4. Slug migration

### 4.1. Preserved slug prefixes

The following slug prefixes are unchanged. Every `§{prefix.leaf}` reference in source continues to work, and every anchored URL `/<path>/#prefix.leaf` continues to resolve:

- `intro.*`, `notation.*`, `lexical.*` (except `lexical.placeholders`, which moves)
- `recipes.*`, `chores.*`, `xref.*`
- `lua.*` (every sub-slug; only chapter numbers under them change)
- `exec.*` (every sub-slug; only chapter numbers under them change)
- `grammar-appendix.*`, `rationale.*`, `examples.*`, `changes.*`

### 4.2. Retired slug prefixes

The following slug prefixes are retired. Every retired slug must be rewritten in source (the build emits a hard error) and every retired anchored URL is redirected to its replacement:

| Retired | Replacement | Reason |
|---|---|---|
| `grammar.*` | `toplevel.*` (§4), `decl.*` (§5), or `steps.dispatch` (§8) per the sub-slug's topic | the old §3 covers material now spread across three chapters |
| `modules.*` | `comp.*` (§11) or `mods.*` (§12) per the sub-slug's topic | the old `modules` slug conflates cross-Cookfile composition with the `use` system; both halves rename to eliminate the silent meaning-shift |
| `stdmods.*` | `cat.*` (§27) or `cat.cc.*` (§28) | shorter prefix that scales per-module |
| `lexical.placeholders` | `phl.token` | placeholders consolidated into §9 |

### 4.3. New slug prefixes

| Prefix | Chapter | Holds |
|---|---|---|
| `conf.*` | Ch. 1 | Conformance |
| `toplevel.*` | Ch. 4 | Top-level production, ordering, termination rule, top-level module_call |
| `decl.*` | Ch. 5 | `use`/`import`/`config`/`register` declaration grammars |
| `steps.*` | Ch. 8 | Step kinds and step dispatch |
| `phl.*` | Ch. 9 | Placeholder token, resolution, per-context tables |
| `comp.*` | Ch. 11 | Cross-Cookfile composition |
| `mods.*` | Ch. 12 | The `use` system, module resolution, catalogue index |
| `cat.*` | §27, §28+ | Standard module catalogue |
| `pre-v1.*` | Annex D | Pre-1.0 checklist (currently slug-less) |
| `corpus.*` | Annex F | Embedded conformance corpus (stub) |

### 4.4. Slug-rename registry

`scripts/slug-renames.ts` is a new file holding the one-way map from retired slug to replacement. It drives two mechanisms:

1. **Build-emitted redirects.** `astro.config.mjs` reads the registry and emits a Starlight redirect for every retired anchor. Existing URLs continue to resolve to the new chapter.
2. **Source-ref lint.** `src/plugins/remark-slug-xrefs.ts` consults the registry on a missing slug. If a source file uses a retired slug, the build fails with a precise error: `§{grammar.recipe-syntax} renamed to §{recipes.header-forms}`. This prevents the deprecated names from creeping back into source.

## 5. Per-chapter contents migration

This section enumerates every source location whose new home differs from a one-to-one chapter shift.

### 5.1. Cross-Part moves (the consequential migrations)

| Material today | New home | Rationale |
|---|---|---|
| §2.11 Placeholders in shell text | §9 Placeholders | placeholder behaviour is a Cookfile-language concern that touches lexical, resolution, and per-step contexts; consolidating into one chapter eliminates the four-place split |
| §3.9 Step dispatch cascade | §8 Step kinds (single normative source) | currently restated in §4.4 and App. A.4 |
| §3.7.5 Top-level module_call | §4 Top-level structure | it is a top-level construct, not a register-block sub-topic |
| §4.1.1 Body termination | §4 Top-level structure | the termination rule applies to recipe body, config body, and register-block body uniformly |
| §4.4 Note 4.4.2 region rule | §6 Recipes (promoted to a numbered subsection) | this is a recipe-shape rule, not a step rule, and the "Note" wrapper understates its normative force |
| §4.9.3 Body bundling | §15 Step groups (Part II) | bundling is execute-phase behaviour, not recipe surface |
| §6.2.1 `discovered_inputs` field syntax | §22 Register-phase API; **semantics move to §17 Cache** | textbook syntax-vs-semantics split |
| §6.3.1 `cook.sh` | §24 Both-phase API | already documented as both-phase; now lives with the rest of the both-phase surface |
| §6.3.4 `cook.load_module` | §24 Both-phase API; **module lifecycle moves to §12 Modules** | the API call and the module lifecycle are separable concerns |
| §6.4 Using-block globals | §23 Execute-phase API | already documented as execute-phase; gets a proper home |
| §6.7 / §6.7.1 placeholder tables | §9 Placeholders | part of the consolidation |
| §6.8 The `use` declaration | §12 Modules | top-level Cookfile construct, not a Lua API call |
| §7.6 Workspace root | §20 Workspace root (Part II) | invocation-time concern, not a Cookfile-language concern |
| §7.7 Cache portability invariants | §17 Cache semantics (Part II) | belongs with the rest of cache semantics |
| §9.1 Catalogue bootstrap, vendoring, index | §27 Catalogue governance | catalogue governance pulls out of the `cc`-specific material so future modules can be added cleanly |
| §9.2 `cc` module | §28 `cc` — C-family build module (own file) | one `.mdx` per blessed module |

### 5.2. Slimmed-down chapters

Several chapters shrink because material moves out:

- **Chapter 3 Lexical structure** loses §2.11 (placeholders).
- **Chapter 6 Recipes** retains header, dependency list, body shape, and the promoted region rule; ingredients-step body and all other step kinds live in §8.
- **Chapter 11 Cross-Cookfile composition** loses workspace root (to §20) and cache portability invariants (to §17).
- **Chapter 12 Modules** absorbs material from §6.8, §9.1, and module-lifecycle prose from §6.3.4.

### 5.3. Annex C (Rationale) handling

Rationale slugs (`rationale.X`) are preserved. Heading numbers within the annex are renumbered to match the new chapter map (`B.2.2` → `C.3.2`, etc.). Because rationale is reached by slug, the heading-number changes are cosmetic; every `§{rationale.X}` reference in prose continues to resolve.

## 6. New normative prose required by the reorg

The reorg surfaces three material gaps in the current Standard. All three land in the same cut.

### 6.1. §24 Both-phase API — formal definitions for previously-amended surfaces

Current §6.3 Note 6.3.1 disclaims: "Additional `cook.*` helpers — notably `cook.env`, `cook.platform`, `cook.cache`, `cook.export`, `cook.import`, `cook.dep_output`, and `cook.dep_output_list` — are part of the runtime but are specified in §{modules} (modules and configuration), not here." In practice these surfaces are not fully specified in §7 either; they are sketched in CS-0066/0070/0071 amendments to §6.3.4.

With the Lua API as its own Part, the defer-to-elsewhere stops working. §24 introduces formal subsections for:

- `cook.env` — read/write, the namespace, interaction with placeholders.
- `cook.cache.get(key)`, `cook.cache.set(key, value)`, `cook.cache.scope(label)` — phase availability, persistence model, scope semantics.
- `cook.export(name, info)`, `cook.import(name)` — phase availability, namespace, interaction with module loading.
- `cook.platform.*` — the platform-detection surface.
- `cook.dep_output(name)`, `cook.dep_output_list(name)` — resolution and phase availability.

Estimated size: ~120 normative lines. The `cook.cache` and `cook.export`/`cook.import` subsections consolidate and formalise behaviour pinned in CS-0070 and CS-0071 respectively; the `cook.env`, `cook.platform`, `cook.dep_output`, and `cook.dep_output_list` subsections formalise long-standing surfaces that have never had a dedicated normative home but whose behaviour is referenced throughout the existing Standard. No new normative requirements are introduced that an existing implementation would fail to meet.

### 6.2. §12 Modules — explicit lifecycle section

Current spec scatters module lifecycle across §6.3.4 (`cook.load_module`), §6.8 (use declaration), and §7.4 (use scope). The new §12 introduces a dedicated lifecycle subsection covering:

- When a module's top-level chunk executes (load phase, before any register phase).
- When `init()` executes (once per VM, after top-level, before any recipe registration).
- Per-VM caching semantics (the `(working_dir, name)` key) and its interaction with cross-Cookfile composition.
- The register/execute-phase API surface split as it applies to module bodies.

Estimated size: ~40 normative lines. Material is gathered from existing scattered prose.

### 6.3. §17 Cache — absorption of §7.7

§7.7 cache portability invariants move into §17. A short cross-reference note remains in §11 Composition for navigation.

## 7. Build and tooling impact

| Touchpoint | Change |
|---|---|
| `scripts/slug-mapping.ts` | Update every slug's section number to the new map. Add the new slug prefixes from §4.3. Remove `grammar.*`, `modules.*`, `stdmods.*` entries — these are now covered by the rename file. |
| `scripts/slug-renames.ts` *(new)* | One-way map from retired slug to replacement, per §4.4. |
| `astro.config.mjs` | Sidebar rewrite into four collapsible Parts plus Front matter and Annexes. Generate `redirects` entries from `slug-renames.ts`. |
| `src/plugins/remark-slug-xrefs.ts` | On a missing slug, consult `slug-renames.ts` and emit a build error naming the new spelling. |
| `src/plugins/__tests__/` *(two new tests)* | (1) every old slug in `slug-renames.ts` resolves to a present slug in `slug-mapping.ts`; (2) no `§{X}` reference in `src/content/docs/**/*.mdx` uses a retired slug. |
| `src/plugins/clauses.ts`, `cs-ids.ts`, `rehype-bare-ref-lint.ts`, `remark-rfc2119.ts`, `remark-cook-highlight.ts` | No changes. |
| `conformance/` | No changes. |
| `cook_modules/checks.lua` | No changes. |

## 8. Migration mechanics

The reorg lands in one PR composed of six logical commits.

### 8.1. Step 1 — Pre-flight infra

- Add `scripts/slug-renames.ts` with the full retired-slug map.
- Wire `astro.config.mjs` redirects and the slug-xref plugin update.
- Add the two new plugin tests. They fail until Step 2 completes.

### 8.2. Step 2 — Author the new structure in parallel files

Create the new content. New files only — existing files are untouched at this step so the diff is purely additive.

- Front matter: `00-introduction.mdx` (trimmed), `01-conformance.mdx` (new from §0.7), `02-notation.mdx` (renamed from `01-notation.mdx`).
- Part I: `03-lexical.mdx` through `12-modules.mdx`.
- Part II: `13-two-phase.mdx` through `20-workspace.mdx`.
- Part III: `21-lua-api.mdx` through `26-path.mdx`. Includes the new normative prose from §6.
- Part IV: `27-catalogue.mdx`, `28-cc.mdx`.
- Annexes: rename and reorder per §3.6; add `F-corpus.mdx` stub.

### 8.3. Step 3 — Delete the old structure

Once the new files exist and reference each other correctly, delete the old MDX files. `git mv` is used where chapter content survives substantially intact (e.g., `01-notation.mdx` → `02-notation.mdx`) so blame is preserved cleanly.

### 8.4. Step 4 — Finalise `slug-mapping.ts`

New chapter numbers wired in. Old slug entries deleted; they live only in `slug-renames.ts` from this point forward.

### 8.5. Step 5 — Record the CS entry

- Add the CS-NNNN entry to `E-changes.mdx` (the new home of the changelog) per the template in §10.
- Bump `VERSION` to `0.10` in the same commit.
- Tag `cs-standard/v0.10`.

## 9. Validation gates

All gates must pass before merge.

| Gate | Command | What it proves |
|---|---|---|
| Plugin tests | `pnpm test` | All slug refs resolve; no retired slugs in source; redirect targets exist |
| Site build | `pnpm build` | All MDX renders; redirect map emitted; sidebar renders four Parts |
| Conformance harness | `cargo test -p cook-lang --test conformance` | Reorg has not drifted the corpus contract |
| Keyword lint | `cook standard.lint` | All new normative prose uses RFC 2119 keywords correctly |
| Against-tag harness | `cook standard.against-tag cs-standard/v0.9` | Backwards conformance against the prior tagged Standard parses. Per App. E.1 this is allowed to break on parser-impl drift; document if it breaks here. |
| Redirect smoke test | manual: ten representative retired URLs | Old `/02-lexical/#grammar.recipe-syntax` lands at new `/06-recipes/#recipes.header-forms` |
| Cross-doc refs | `grep -r '§{grammar\.\|§{modules\.\|§{stdmods\.' src/content/docs/` | Zero hits |

## 10. CS-NNNN entry template

The reorg lands as a single CS entry in the new `E-changes.mdx`:

```markdown
## CS-NNNN — Structural redesign: Parts and per-topic chapters

**Date:** YYYY-MM-DD
**Version:** v0.10
**Sections affected:** entire Standard (reorganisation). Specifically:
  - Renumbered Chapters 0–9 → Chapters 0–28 across four Parts.
  - Retired slug prefixes: `grammar.*`, `modules.*`, `stdmods.*`.
  - New slug prefixes: `conf.*`, `toplevel.*`, `decl.*`, `steps.*`,
    `phl.*`, `comp.*`, `mods.*`, `cat.*`, `pre-v1.*`, `corpus.*`.
  - New normative prose: §24 Both-phase API definitions for
    `cook.env`, `cook.cache`, `cook.export`, `cook.import`,
    `cook.platform`, `cook.dep_output`, `cook.dep_output_list`;
    §12 Modules lifecycle section.
  - Annexes D and E swapped (pre-1.0 checklist now precedes Changes).
  - Annex F (Conformance corpus) added as a stub.
**Summary:** The Standard is reorganised into four Parts —
  The Cookfile language (Part I, author-facing), Execution model
  (Part II), The Cook Lua API (Part III), and Standard module
  catalogue (Part IV) — with each chapter focused on one topic.
  Phase classification, cache semantics, and placeholder
  specifications each have a single normative home, eliminating
  the prior three-way splits. The `use` declaration moves from
  the Lua API chapter to Modules. Chores becomes a real chapter.
  Workspace root and cache portability move into the Execution
  model. Body bundling moves from the recipes chapter into step
  groups. Step dispatch has one normative source. The ~160-line
  filler in Part III formalises Cook Lua surfaces previously
  specified only in CS-0066/0070/0071 amendments.
  No normative behaviour changes for any conforming Cookfile;
  this entry is structural and editorial only. The conformance
  corpus is unchanged. Retired anchored URLs continue to resolve
  through build-emitted redirects.
**Reference:** this commit.
```

## 11. Risk register

| Risk | Mitigation |
|---|---|
| Reorg PR is huge and hard to review | Six logical commits within the PR: Pre-flight infra, Part I content, Part II content, Part III content, Part IV content, Annexes plus cleanup. |
| A slug rename misses a source reference | Plugin test (gate 1 in §9) fails the build until every retired slug in source is rewritten. |
| Filler prose in §24 and §12 reads as a separate concern | The CS entry explicitly anchors the filler to CS-0066/0070/0071, naming each amendment whose informal specification this formalises. |
| Backwards-conformance harness breaks | Pre-existing risk per App. E.1. Acceptable per current operating rule; document in the CS entry if it occurs. |
| External users have bookmarks to the old structure | Build-emitted redirects make every old anchored URL still resolve. |
| `09-standard-modules.mdx` becomes `28-cc.mdx` — git blame loss | Use `git mv` then in-file edits in separate commits so blame is preserved cleanly. |
| Annex C (Rationale) heading renumber confuses readers | Slugs are preserved; every `§{rationale.X}` reference continues to resolve. The heading-number shift is cosmetic. |

## 12. Out of scope

The following items are deliberately not part of this reorg. Each is a separate workstream tracked under its own future CS entry:

- Tree-sitter parity (CS-0002 follow-up).
- Backwards-conformance harness brittleness (App. E.1).
- Any addition to the blessed module catalogue beyond `cc`.

## 13. Acceptance criteria

A reviewer can sign off on the reorg PR when:

1. All seven validation gates from §9 pass.
2. Every retired slug from `slug-renames.ts` has a redirect target that resolves.
3. The git history within the PR is structured into six logical commits per §8.
4. The CS-NNNN entry is in place in `E-changes.mdx` and the `VERSION` bump to `0.10` is in the same commit.
5. The pre-commit hook (`.githooks/pre-commit`) passes.
6. `cook standard.lint` reports zero new RFC 2119 violations.
7. A reviewer can navigate from the rendered TOC to every numbered section and from every numbered section back to its annexed rationale.
