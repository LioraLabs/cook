# Slug-based cross-references for the Cook Standard

**Status:** design approved 2026-04-24
**Scope:** `standard/` (spec prose + build plugins); no Rust, no conformance changes

## 1. Problem

Cross-references in the Standard are currently positional: authors write `§ 2.3`
in prose, the rehype pipeline synthesises anchor IDs of the form `sec-2-3` from
heading numbers, and link resolution matches the two by section number.

Two recent commits (`35d46d9`, `e83c490`) fix stale refs left behind after the
CS-0009 restructure renumbered several sections. The existing
`rehype-clause-xrefs` plugin silently falls back to plain text for unresolved
refs (plugin comment: *"Keep unresolved refs as plain text — do not error"*),
so drift accumulates without build-time signal. § 1.7 *"Stable anchors"* of
the Standard already claims anchors survive renames; the current mechanism
does not deliver that claim.

A prose survey finds 317 `§` references across the five normative chapters
and four appendices. With roughly 60 numbered clauses in total, the mapping
from author-stable identifiers to rendered numbers is small and one-to-many
(many refs, few clauses) — a shape that rewards investing once in stable
identifiers.

## 2. Goals and non-goals

### Goals

1. Renumbering a section never breaks a ref.
2. Renaming a heading's title never breaks a ref.
3. Unresolved refs fail the build loudly, naming the source file and line.
4. Readers see the same `§ N.M` numeric citation they see today, with the
   same link behaviour and tooltip.
5. Anchor URLs (`/02-lexical/#...`) are human-readable and stable across
   edits that do not rename the clause itself.

### Non-goals

- Changing what readers see. No slug text appears in rendered prose.
- Changing the conformance suite or any Rust code.
- Stabilising *example* numbering (`Example 2.3.1`). Examples are cited by
  number in prose today and retain that form; if stable example refs are
  needed later, they are an additive follow-up.
- Back-compat with existing `sec-N-M-K` anchor URLs. The Standard is
  pre-release and has no external consumers.

## 3. Design

### 3.1. Slug grammar

A slug is `<chapter>.<leaf>`, with each segment matching `[a-z][a-z0-9-]*`.
Dots delimit the namespace; dashes delimit words within a segment. Two
levels total — no nested namespaces. Deeper clauses (`§ 5.4.1`) use a
descriptive leaf (`xref.dep-driven-iteration-recipe-output`) rather than a
three-level slug; this keeps the grammar flat and forces authors to name
the clause, not its position.

Slugs are globally unique. Chapter prefixes are drawn from a fixed set and
recorded in § 1.7 of the Standard:

| Chapter                               | Prefix             |
| ------------------------------------- | ------------------ |
| 0. Introduction                       | `intro`            |
| 1. Notation and conventions           | `notation`         |
| 2. Lexical structure                  | `lexical`          |
| 3. Syntactic grammar                  | `grammar`          |
| 4. Recipes and step kinds             | `recipes`          |
| 5. Cross-recipe references            | `xref`             |
| 6. Cook Lua API                       | `lua`              |
| 7. Cross-Cookfile composition         | `modules`          |
| 8. Execution model                    | `exec`             |
| A. Grammar (normative appendix)       | `grammar-appendix` |
| B. Rationale (informative)            | `rationale`        |
| C. Examples (informative)             | `examples`         |
| D. Changes                            | `changes`          |

Chapter-level clauses (the `# N. Title` heading itself) use the bare prefix
as their slug, e.g. `{#lexical}`. A citation `§{lexical}` then renders
`§ 2`.

### 3.2. Heading declaration

Each numbered heading in the spec carries its slug as an MDX/remark
attribute directive:

```mdx
## 2.3. Identifiers. {#lexical.identifiers}
```

Remark parses `{#...}` directives natively and hoists them onto the
heading's `data.hProperties.id`. The existing `rehype-clause-anchors`
plugin stops synthesising IDs from heading numbers and instead validates
that each numbered heading has a directive-supplied ID matching the slug
grammar.

### 3.3. Source citation syntax

Authors write `§{chapter.leaf}` in prose. Examples:

```mdx
See §{lexical.identifiers} for the keyword list and §{xref.names} for
how names are resolved across recipes.
```

The xref plugin rewrites each `§{slug}` to an HTML anchor:

```html
<a href="/02-lexical/#lexical.identifiers"
   class="clause-xref"
   title="2.3. Identifiers">§ 2.3</a>
```

The rendered text (`§ 2.3`) is pulled from the heading's live number,
not from the source. When a section renumbers, every rendered ref
updates on the next build without a prose diff.

Appendix refs use the same syntax: `§{grammar-appendix.steps}` →
`§ A.4`. The citation prefix (`§` vs `App.` in the current Standard) is
normalised on output; the design keeps `§` for both because the rendered
number carries enough information to distinguish chapters from
appendices, and because the `§{...}` source form already signals
"cross-reference" unambiguously.

### 3.4. HTML anchors

Each numbered heading emits `<hN id="<slug>">` as its sole anchor. HTML5
permits `.` in ID values and in URL fragments; no encoding is needed.
CSS selectors are unaffected because the Standard's stylesheets target
headings by tag and class, not by ID.

The previous `sec-N-M-K` anchors are removed. No alias is emitted.

### 3.5. Validation

All five checks are added to the build and produce hard errors:

1. **Every numbered heading has a slug directive.**
   A heading whose text matches the clause regex (`N[.M[.K]]. Title`) must
   carry a `{#...}` directive. Missing directive → error with file, line,
   and heading text.
2. **Slug format.**
   Directive value must match `[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)?`. Else
   error.
3. **Chapter prefix.**
   The first segment must be one of the registered chapter prefixes
   (§ 3.1). Else error.
4. **Uniqueness.**
   Duplicate slug → error naming both source locations (replaces the
   current duplicate `sec-N-M-K` check).
5. **All `§{...}` refs resolve.**
   Unknown slug → error naming the source file, line, and slug.

A lint rule rejects bare `§ N.M` or `§ N` in prose outside code blocks.
This prevents accidental regressions after migration. Inside fenced code,
inline code spans, and existing code-block anchor syntax, numeric `§`
forms remain legal (they are not refs).

### 3.6. Spec text updates

Two clauses in Chapter 1 are rewritten:

- **§ 1.2 "Section numbering and citation"** gains a paragraph describing
  the source-form `§{chapter.leaf}` and clarifying that rendered prose
  shows the numeric form.
- **§ 1.7 "Stable anchors"** is rewritten as the canonical description of
  the slug system: grammar, chapter-prefix table, the requirement that
  slugs survive renumbering and retitling, and the amendment rule
  (changing a slug requires a D-changes entry).

A new D-changes entry (CS-NNNN, allocated during implementation) records
the change from positional to slug-based anchors.

## 4. Implementation

### 4.1. Plugin changes

Four files under `standard/src/plugins/`:

- **`rehype-clause-anchors.ts`** — read `{#...}` directive from the
  heading's `data.hProperties.id`; enforce checks 1–4 (§ 3.5); remove
  `sec-N-M-K` synthesis.
- **`clauses.ts`** — harvest slugs instead of numeric anchors. Key the
  `ClauseInfo` map by slug; store both `href` (route + `#<slug>`) and the
  heading's current numeric text (`"2.3"`) for render-time substitution.
- **`rehype-clause-xrefs.ts`** — regex becomes `§\{([a-z0-9.-]+)\}`;
  unresolved → throw (check 5). Render-text pulled from `ClauseInfo`.
- **(new) `rehype-bare-ref-lint.ts`** — scan text nodes (same ancestor
  rules as the xref plugin: skip `code`/`pre`/`a`) for `§ N[.M[.K]]`;
  fail with a remediation message pointing at the slug registry.

Existing `__tests__` are updated; new tests cover each failure mode and
the slug-render round-trip.

### 4.2. Migration script

A one-shot Node script under `standard/scripts/` (e.g. `migrate-slugs.mjs`):

1. Walks `src/content/docs/**/*.mdx`.
2. For each numbered heading, looks up the current `sec-N-M-K`, applies
   a pre-baked `sec-id → slug` mapping table (committed alongside the
   script for review), and appends the `{#slug}` directive.
3. For each prose ref of the form `§ N[.M[.K]]`, looks up the slug via
   the same table and rewrites to `§{slug}`.
4. Writes the updated MDX files.

The mapping table is hand-authored as part of the implementation plan —
it is the one place where slug choices are reviewed. Running the script
is a mechanical rewrite after the table is agreed.

The migration script is retained in-tree after the one-shot run because
the mapping table is also the authoritative source for the chapter
prefix list and the first slug of each clause; future renames update the
table alongside the spec.

### 4.3. Order of operations

Single branch, single CS identifier, small number of commits:

1. Plugin changes + tests (build-breaking without migration — held on
   branch until step 3).
2. Migration script + mapping table; dry-run diff reviewed.
3. Migration script applied; all headings and refs updated in one commit.
4. § 1.2 / § 1.7 rewrites + D-changes entry.

Steps 1–3 land together on merge; step 4 may be a separate commit on the
same branch for review clarity.

## 5. Risks and tradeoffs

- **Authoring cost.** Every new clause now requires a slug decision.
  Mitigated by: the chapter prefix is fixed, the leaf is a short
  kebab-case phrase, and the build fails loudly if the author forgets.
  Net cost per clause: one line of thought, one directive.
- **Slug-renaming churn.** Renaming a slug is a breaking change for
  every ref that points at it. Mitigated by: D-changes entry rule
  (§ 3.6), plus the "loud" validation catching all affected refs on the
  next build — no silent rot.
- **Readability in source.** `§{xref.dep-driven-iteration-recipe-output}`
  is longer than `§ 5.4.1`. This is the explicit tradeoff; the whole
  point is that the source now encodes *what* the ref points at, not
  *where* it happens to live. Leaves are chosen short where possible.
- **Collisions with MDX directive syntax.** `{#...}` at end of heading
  is standard remark-directive syntax and does not conflict with MDX
  JSX expressions elsewhere in the file. Verified by the existing
  heading shapes in `src/content/docs/`.

## 6. Open questions

None blocking. The following are deferred:

- Whether example refs (`Example 2.3.1`) should also migrate. Revisit
  once the clause migration ships; the same mechanism extends trivially
  if needed.
- Whether `App. X` in prose (distinct from `§` for appendices) should be
  folded into the `§{...}` form too. The current design accepts `§{...}`
  for appendices and renders `§ A.N`; if the `App.` prefix is retained
  in some contexts, a render-time option on `§{...}` can select it. Left
  for the implementation plan to decide based on audit of current `App.`
  usage.
