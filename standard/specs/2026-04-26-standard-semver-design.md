# Design: Cook Standard versioning, D-changes reshape, conformance-claim convention

**Date:** 2026-04-26
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time; the body of this design refers to it as CS-0012 for readability)
**Scope:** Cook Standard authoring surface only. No Cookfile-language behaviour changes. No normative obligations on implementations beyond clarifying what "conforms to v0.X" means.
**Predecessor:** CS-0011 (this design completes the follow-up tracked in the CS-0011 design's §2 and §4 "Out-of-scope items surfaced during brainstorm" lists, and in `appendix/D-changes.mdx` D.11 "Implementation status").

## 1. Motivation

The Cook Standard is currently an unversioned head-of-`main` document. §0.5 (`intro.version`) states the posture and notes that dual-track versioning is contemplated for a future 1.0 release. Three forces motivate introducing pre-1.0 versioning now rather than at 1.0:

1. **Implementations need a stable referent.** CS-0002 (planned) will bring `tree-sitter-cook` into conformance with "the Standard." When `cook-lang` and `tree-sitter-cook` claim conformance, they need to name *what* they conform to. Today the only available referent is "head of `main`," which drifts under them between merges.
2. **`appendix/D-changes.mdx` is unstructured.** It is a flat chronological list of CS-NNNN entries (CS-0001 through CS-0011 at time of writing). A reader cannot ask "what changed between two points in time" without bisecting the list against git. Grouping CSes under tagged versions answers that question directly.
3. **The development-aide value is realisable now.** Even pre-1.0, naming versions lets PR descriptions, README badges, and follow-up CS bodies refer to "v0.2" instead of "the Standard at commit `abc123`." The cost is small (one VERSION file, two git tags, an Appendix D index, and ~four prose updates); the value is immediate.

## 2. Non-goals

- **Cookfile-side version pragmas.** A `#! cook 0.2` header at the top of a Cookfile would let the parser do something useful only if there were multiple supported Standard versions in flight. Pre-1.0 lockstep means there is exactly one supported version at any time. Deferred to a 1.0-era CS.
- **Versioned conformance corpus directories.** Restructuring `standard/conformance/` into `conformance/v0.1/`, `conformance/v0.2/`, … duplicates information that git already snapshots at the tag, creates a forward maintenance burden (back-porting fixes, deciding when an old directory is "frozen"), and is over-built for a project with one implementation under active development.
- **Normative implementation-claim mechanism.** Mandating that an implementation MUST expose its claimed version through a CLI flag, library constant, or specific API surface conflicts with §0.3 (which excludes the CLI from Standard scope) and adds normative MUSTs that no automated check can enforce pre-1.0. Convention-only is the right strength.
- **PATCH track and 1.0 transition rules.** Pre-1.0, breaking changes are normal and PATCH buys nothing. The rules for transitioning to strict SemVer (MAJOR.MINOR.PATCH) at 1.0 will be designed when 1.0 is on the horizon, not preemptively.
- **CLI surface for `cook --standard-version`.** §0.3 excludes the CLI. The convention layer (CONTRIBUTING.md) is free to suggest CLI surfaces; the Standard does not require them.
- **Back-porting fixes to v0.1.** No support window pre-1.0. If a v0.1 corpus error is found, fix it on head and let it ship in the next cut. The v0.1 tag remains as historical record.
- **Soft Serve release objects or GitHub-style structured release notes.** The git tag is the release. The cook repo is hosted on Soft Serve, not GitHub; release-object affordances are not assumed.
- **Docs-site version switcher.** Rendering multiple tagged versions of the Standard side-by-side on the published site is a tooling concern, not a Standard concern. Deferred.

## 3. Design

### 3.1. Version model

The Standard adopts `MAJOR.MINOR` numbering for the pre-1.0 era. The rules:

- **Numbering shape.** Two integers separated by a dot. No PATCH track pre-1.0. Clarifications and typo fixes that ship without a CS land on head-of-`main` and are absorbed into the next MINOR cut.
- **Bump rule.** MINOR increments on each *cut*. A cut MAY contain one or more CS entries. Authoring granularity (one CS per merge) and publication granularity (one cut per N CSes) are decoupled. There is no rule against a cut containing exactly one CS — it is simply not required.
- **Cut mechanism.** A cut is the conjunction of three actions performed in a single commit on `main`:
  1. Bump `standard/VERSION` (see §3.2).
  2. Add a new entry to the top of the Appendix D **Versions** index (see §3.4).
  3. Tag the commit `cs-standard/vX.Y` and push the tag.

  The tag and the index entry together constitute the published cut. Either alone is incomplete.
- **1.0 transition.** Out of scope for this CS. The transition will introduce strict SemVer (MAJOR.MINOR.PATCH) and presumably a Cookfile-side version pragma; both are deferred. §0.5 will retain language flagging the deferred transition.

### 3.2. New file: `standard/VERSION`

Single-line text file containing the current version string, e.g.:

```
0.2
```

No trailing newline policy beyond "what `git diff` produces by default." The file is the canonical machine-readable source of truth for the head-of-`main` Standard version. Astro reads it at build time and substitutes it into the docs index `Status:` line so the rendered site shows e.g. "v0.2 — head-of-main lockstep, pre-1.0."

The file is *not* Cookfile-language surface; it is Standard metadata. It is not part of the conformance corpus and is not consulted by any conforming implementation at runtime.

### 3.3. §0.5 (`intro.version`) — rewrite

Replace the current §0.5 paragraph with prose defining:

- The numbering scheme (`MAJOR.MINOR` pre-1.0, no PATCH track).
- The location of the canonical version string (`standard/VERSION`, rendered into the docs index).
- The bump rule (MINOR per cut; cuts MAY batch CSes).
- The cut mechanism (`cs-standard/vX.Y` tag + `VERSION` bump + Appendix D index entry, in one commit).
- The retroactive-tagging boundary: pre-CS-0012 history is grouped under v0.1; CS-0011 and CS-0012 ship in v0.2.
- The retained "no version pragma in the Cookfile header at this time" sentence.
- The deferred 1.0 transition statement.

Approximate prose (use as the implementation plan's starting draft; final wording refined at writeup):

> Cook Standard versions are of the form `MAJOR.MINOR`. The current version is recorded in `standard/VERSION` and rendered in the site header. Pre-1.0, MINOR increments on each *version cut*. A cut MAY contain one or more CS entries (App. D); authoring granularity and publication granularity are decoupled. A cut consists of bumping `standard/VERSION`, adding the new version to the top of the Appendix D Versions index, and tagging the commit `cs-standard/vX.Y`, all in one commit on `main`. CS-0001 through CS-0010 are grouped retroactively under v0.1; CS-0011 and CS-0012 ship in v0.2. There is no version pragma in the Cookfile header at this time. At 1.0, the Standard will transition to strict SemVer (MAJOR.MINOR.PATCH) and the rules for that transition will be defined; both are out of scope for the present draft.

### 3.4. §0.7 (`intro.conformance`) — amendment

Add a paragraph defining what *"conforms to Cook Standard v0.X"* means. The existing three numbered points are retained. Approximate prose (use as the implementation plan's starting draft; final wording refined at writeup):

> An implementation is said to **conform to Cook Standard v0.X** when, against the prose and corpus of the `cs-standard/v0.X` tag, it satisfies the three points above. The mechanism by which an implementation claims a version (a README statement, a library constant, a CLI flag, or any other surface) is implementation-defined and is not normatively required by this Standard.

The new paragraph adds one sentence that is informative-by-disclaimer ("not normatively required") and one sentence that is normative-by-virtue-of-RFC-2119-keyword-by-implication ("conforms to v0.X" is now a defined term). No new MUSTs are introduced beyond what §0.7's existing three points already establish; the amendment binds those points to a specific tag rather than to head-of-`main`.

### 3.5. Appendix D (`changes`) — restructure

Three changes:

1. **New top-of-page Versions index.** A new h2 section, immediately after the appendix's existing intro paragraph, listing each cut version with its tag, date span, and CS contents. Newest version first. Shape:

   ```markdown
   ## Versions

   - **v0.2** (`cs-standard/v0.2`, 2026-04-26) — CS-0011, CS-0012
   - **v0.1** (`cs-standard/v0.1`, 2026-04-22..2026-04-24) — CS-0001..CS-0010
   ```

   Each subsequent cut adds one bullet at the top of this list.

2. **Per-CS `**Version:**` line.** Every existing CS body gains a `**Version:** v0.X` line near its existing `**Date**`/`**Sections affected**`/`**Reference**` lines. Placement convention: between `**Date**` and `**Sections affected**`. Backfill values: CS-0001..CS-0010 are `v0.1`; CS-0011 and CS-0012 are `v0.2`.

3. **Per-CS h2 sections retained.** No structural change to the existing per-CS bodies beyond the new `**Version:**` line. Slugs (e.g. `changes.cs-0010`, `changes.cs-0011`) are preserved.

The Versions index serves the version-oriented reader ("what shipped in v0.2?"); the per-CS bodies serve the CS-oriented reader ("what does CS-0009 say?"). Both are useful; the index is cheap.

### 3.6. Appendix B (`rationale`) — new subsection

A new B.0.x subsection titled **"Versioning posture pre-1.0"**, slugged `rationale.versioning-pre-1-0`, captures the four design decisions that are likely to be questioned later:

- **Why `MAJOR.MINOR` not strict SemVer pre-1.0.** Breaking changes are normal pre-1.0; the discipline of distinguishing MAJOR from MINOR is performative when *every* cut may contain a breakage. PATCH buys nothing — there is no installed base whose conformance is broken by a typo fix that nobody has to back-port. Strict SemVer arrives at 1.0 along with an installed base for which the discipline starts to matter.
- **Why version cuts MAY batch CSes.** Authoring (one CS per merge) and publication (one cut per N CSes) are different concerns. Batching keeps the cut cadence at "when an implementation needs to claim a new version" rather than "after every spec edit," which would inflate the version number into the dozens with little reader benefit.
- **Why the conformance-claim mechanism is convention, not normative requirement.** No consumer pre-1.0; §0.3 excludes the CLI surface; no automated check enforces a MUST without an ecosystem to enforce against. The convention is documented in `CONTRIBUTING.md` (see §3.7), where it is amendable without a CS.
- **Why the corpus is identified by git tag rather than versioned subdirectory.** Git already snapshots prose and corpus together at the tag. Versioned subdirectories duplicate that snapshot in a place where it must be manually maintained; they also imply a back-port story that pre-1.0 explicitly disclaims (see §2 non-goal "Back-porting fixes to v0.1").

Slot placement: B.0 currently does not exist (Appendix B is organised by chapter — B.2 maps to §2, B.3 to §3, etc.). Two options for placement, decided during writeup:

- **(a)** Add a new top-level `B.0` "Introduction-chapter rationale" containing only the new versioning subsection. Cleanest mapping; sets a precedent for future §0-related rationale.
- **(b)** Place under `B.1` "Notation rationale" as `B.1.x`, on the grounds that versioning is meta-prose discipline rather than language semantics. Slightly cramped fit.

The plan SHOULD pick (a) unless writing the implementation reveals a concrete reason to prefer (b).

### 3.7. `CONTRIBUTING.md` (repo root) — new subsections

The cook-repo `CONTRIBUTING.md` (already houses the spec-first rule) gains two new subsections:

1. **"Cutting a Cook Standard version."** Procedural recipe, three steps:
   - Bump `standard/VERSION` to the next MINOR.
   - Add the new version to the top of the Appendix D Versions index, listing the CSes it covers.
   - Tag the merge commit `cs-standard/vX.Y` and push.

   Plus a note that all three actions land in one commit on `main`, and that the same commit body should set each batched CS's `**Version:**` line.

2. **"Implementation conformance claims."** Convention statement:
   - Each implementation states its claimed Standard version in its README.
   - For Rust crates (`cook-lang`), the convention is a `pub const COOK_STANDARD_VERSION: &str = "0.2";` in the crate root.
   - For `tree-sitter-cook` (when CS-0002 lands), the convention is a header comment in `grammar.js`.
   - This convention is not normatively required by the Standard; it is a project discipline.

### 3.8. `standard/README.md` — minor link

`standard/README.md` already references `../CONTRIBUTING.md` for the spec-first rule. Add a sibling reference to the new "Cutting a Cook Standard version" subsection so spec maintainers find it from the standard directory.

## 4. Initial-state actions

Performed once, as part of merging CS-0012:

- **Tag `cs-standard/v0.1`** at `git merge-base main feat/cs-0011-remove-vardecl` — the most recent commit on `main` before the CS-0011 branch diverged. v0.1 therefore contains CS-0001..CS-0010 plus any post-CS-0010 prose fixes that landed on `main` before the branch diverged (per §3.1, no-CS clarifications are absorbed into the next cut).
- **Tag `cs-standard/v0.2`** at the CS-0012 merge commit on `main`.
- **Backfill `**Version:**` lines** in the bodies of CS-0001 through CS-0011 (all `v0.1` for CS-0001..CS-0010; `v0.2` for CS-0011). The CS-0012 body sets its own `v0.2` line.
- **Push both tags.**

## 5. Out-of-scope items surfaced during brainstorm

For separate tracking; not addressed here.

- Cookfile-side version pragma at 1.0 and the parser handling it implies.
- A docs-site version switcher rendering multiple tagged versions side-by-side.
- Whether `standard/VERSION` should be machine-parsed by an implementation's build to verify its claimed version matches the in-tree Standard at build time. Tooling concern; deferred.
- Whether `cs-standard/vX.Y` tags should be signed. Project-wide signing policy concern; deferred.

## 6. Review checklist for the implementation plan

When `writing-plans` produces the implementation plan from this design, the plan SHOULD include at least:

- One step per spec file touched: `standard/src/content/docs/00-introduction.mdx` (§0.5 rewrite + §0.7 amendment), `standard/src/content/docs/appendix/D-changes.mdx` (Versions index + per-CS Version lines), `standard/src/content/docs/appendix/B-rationale.mdx` (new B.0.x or B.1.x subsection per §3.6).
- One step for `standard/VERSION` (new file) and the Astro frontmatter / index-template change that surfaces its contents in the rendered docs index `Status:` line.
- One step for `CONTRIBUTING.md` (two new subsections per §3.7).
- One step for `standard/README.md` (link addition per §3.8).
- One step for backfilling `**Version:**` lines in CS-0001..CS-0011 bodies (per §4).
- A verification pass that `rehype-bare-ref-lint` still passes (no broken `§{...}` slugs); the new B.0.x or B.1.x slug is added to the chapter-prefix registry per §{notation.stable-anchors}.
- A verification pass that the rendered site shows the new version in the docs index header.
- A final pre-commit step that runs the conformance harness (`cargo test -p cook-lang --test conformance`) — this CS does not change the corpus, so the harness should pass unchanged; running it confirms the spec-first hook accepts the change.
- Tag commands (per §4) documented as a manual post-merge step, not executed by the implementation plan itself. The plan SHOULD include the exact `git tag` invocations and the `git merge-base` verification command for the v0.1 tag commit.
