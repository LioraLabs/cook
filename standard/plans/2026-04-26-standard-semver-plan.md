# Cook Standard Versioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement CS-0012 (Cook Standard versioning, Appendix D reshape, conformance-claim convention) per `standard/specs/2026-04-26-standard-semver-design.md`.

**Architecture:** Authoring-surface change only. New canonical `standard/VERSION` file is imported into `index.mdx` at build time; §0.5 is rewritten and §0.7 amended in `00-introduction.mdx`; Appendix D gets a top-level Versions index plus a `**Version:**` line per CS body; Appendix B gets a new B.0.1 rationale subsection; `CONTRIBUTING.md` documents the cut procedure and the implementation conformance-claim convention. Post-merge tags `cs-standard/v0.1` and `cs-standard/v0.2` are documented as a manual step.

**Tech Stack:** Astro + Starlight (MDX content), Vite (`?raw` imports), TypeScript (slug registry), pnpm (build/lint runner), `cook-lang` Cargo crate (conformance harness).

---

## Pre-flight

- [ ] **Step 0.1: Verify clean working tree on the right branch**

```bash
git status
git rev-parse --abbrev-ref HEAD
```

Expected: working tree clean (or only the in-progress plan file uncommitted), branch `feat/cs-0011-remove-vardecl` (CS-0012 ships as a follow-up commit on this branch alongside CS-0011 per the design's §1 / Predecessor field).

- [ ] **Step 0.2: Verify the design doc is committed**

```bash
git log --oneline -1 standard/specs/2026-04-26-standard-semver-design.md
```

Expected: a `design(standard): Cook Standard versioning, ...` commit shows.

- [ ] **Step 0.3: Verify the build is green before any edits**

```bash
cd standard && pnpm build && cd ..
```

Expected: build completes without errors. If it fails, stop and surface the failure — the plan assumes a green baseline.

---

## Task 1: Create `standard/VERSION` and wire it into the docs index

Create the canonical version file and surface it in the rendered site header so subsequent tasks have a working version-substitution mechanism to validate against.

**Files:**
- Create: `standard/VERSION`
- Modify: `standard/src/content/docs/index.mdx` (top-of-file frontmatter import + Status line)

- [ ] **Step 1.1: Create `standard/VERSION` with the v0.2 string**

Create `standard/VERSION` with exactly this content (one line, trailing newline):

```
0.2
```

Verify:

```bash
cat standard/VERSION
```

Expected output: `0.2` followed by a newline.

- [ ] **Step 1.2: Modify `standard/src/content/docs/index.mdx` to import VERSION and substitute it into the Status line**

The current top of the file is:

```mdx
---
title: The Cook Standard
description: The authoritative specification of the Cookfile language.
---

# The Cook Standard

**Status:** Draft — head-of-main lockstep, pre-1.0.
**Source of truth:** the Rust parser in `cli/crates/cook-lang/`.
```

Replace it with:

```mdx
---
title: The Cook Standard
description: The authoritative specification of the Cookfile language.
---

import standardVersion from '../../../VERSION?raw';

# The Cook Standard

**Version:** v{standardVersion.trim()} (head-of-main; see [App. D Versions index](/appendix/d-changes/#versions)).
**Status:** Draft — pre-1.0, head-of-main lockstep.
**Source of truth:** the Rust parser in `cli/crates/cook-lang/`.
```

Note the relative import path: `index.mdx` is at `standard/src/content/docs/index.mdx`; three `..` segments resolve to `standard/`, then `VERSION`.

- [ ] **Step 1.3: Run the site build and confirm the version renders**

```bash
cd standard && pnpm build
```

Expected: build completes without errors. Then verify the version is present in the rendered HTML:

```bash
grep -r 'v0\.2' dist/index.html | head -3
cd ..
```

Expected: at least one match showing the rendered Status line containing `v0.2`. If the build fails on the import, double-check the relative path (`../../../VERSION?raw`) and that `standard/VERSION` exists with content `0.2`.

- [ ] **Step 1.4: Commit**

```bash
git add standard/VERSION standard/src/content/docs/index.mdx
git commit -m "$(cat <<'EOF'
spec(standard): add VERSION file, surface in docs index header

Canonical machine-readable version source. Imported into index.mdx via
Vite ?raw query so the rendered Status line shows the current version.

Part of CS-0012.
EOF
)"
```

---

## Task 2: Rewrite §0.5 and amend §0.7 in `00-introduction.mdx`

Define what a Cook Standard version is and what "conforms to v0.X" means.

**Files:**
- Modify: `standard/src/content/docs/00-introduction.mdx` (§0.5 rewrite, §0.7 amendment)

- [ ] **Step 2.1: Replace §0.5 (`intro.version`)**

Find the existing §0.5 block, currently:

```mdx
## 0.5. Version stance [#intro.version]
The Standard tracks the state of `main`. There is no version pragma in the Cookfile header at this time. A future Cook 1.0 release could introduce dual-track versioning (tagged snapshots alongside head-of-main); this is out of scope for the present draft.
```

Replace it with:

```mdx
## 0.5. Versioning [#intro.version]
Cook Standard versions are of the form `MAJOR.MINOR`. The current version is recorded in `standard/VERSION` and rendered in the site header. Pre-1.0, MINOR increments on each *version cut*. A cut MAY contain one or more CS entries (App. D); authoring granularity (one CS per merge) and publication granularity (one cut per N CSes) are decoupled. There is no PATCH track pre-1.0; clarifications and typo fixes that ship without a CS land on head-of-`main` and are absorbed into the next cut.

A cut consists of three actions performed in a single commit on `main`: bumping `standard/VERSION`, adding the new version to the top of the App. D Versions index, and tagging the commit `cs-standard/vX.Y`. The tag and index entry together constitute the published cut.

CS-0001 through CS-0010 are grouped retroactively under v0.1; CS-0011 (top-level `variable_declaration` removal) and CS-0012 (this versioning machinery) ship in v0.2.

There is no version pragma in the Cookfile header at this time. At 1.0, the Standard will transition to strict SemVer (`MAJOR.MINOR.PATCH`); the rules for that transition are out of scope for the present draft.
```

- [ ] **Step 2.2: Amend §0.7 (`intro.conformance`) with the version-claim paragraph**

Find the §0.7 block ending at the "See §{modules} of the Standard (Modules) for additional module-specific conformance requirements." line. After that line, insert:

```mdx

An implementation is said to **conform to Cook Standard v0.X** when, against the prose and corpus of the `cs-standard/v0.X` tag, it satisfies the three points above. The mechanism by which an implementation claims a version (a README statement, a library constant, a CLI flag, or any other surface) is implementation-defined and is not normatively required by this Standard.
```

Do not change the existing numbered list, the intro paragraph, or the cross-reference to §{modules}.

- [ ] **Step 2.3: Run the site build and the keyword lint**

```bash
cd standard && pnpm build && pnpm lint:keywords && cd ..
```

Expected: both succeed. The keyword lint flags lowercase `must`/`shall`/`should`/`may` in normative chapters; the new prose uses uppercase MUST/MAY where binding and lowercase only in informative-by-disclaimer phrasing ("is implementation-defined and is not normatively required" — descriptive, not binding). If the lint flags anything, review and either uppercase or reword.

- [ ] **Step 2.4: Commit**

```bash
git add standard/src/content/docs/00-introduction.mdx
git commit -m "$(cat <<'EOF'
spec(standard): rewrite § 0.5 versioning, amend § 0.7 with version-claim paragraph

§ 0.5 now defines MAJOR.MINOR pre-1.0, the cut mechanism (VERSION
bump + App. D index entry + cs-standard/vX.Y tag), and the v0.1/v0.2
retroactive boundary.

§ 0.7 adds a paragraph defining what "conforms to Cook Standard v0.X"
means (against the cs-standard/v0.X tag) and that the claim mechanism
is implementation-defined.

Part of CS-0012.
EOF
)"
```

---

## Task 3: Add B.0.1 rationale subsection and register its slug

Capture the four design decisions (why MAJOR.MINOR, why batched cuts, why convention-only claims, why git-tagged corpus).

**Files:**
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx` (replace B.0 placeholder)
- Modify: `standard/scripts/slug-mapping.ts` (register `rationale.versioning-pre-1-0`)

- [ ] **Step 3.1: Replace the B.0 placeholder in `B-rationale.mdx`**

Find the current B.0 block:

```mdx
## B.0. On §{intro} Introduction [#rationale.intro]
_To be filled in._
```

Replace with:

```mdx
## B.0. On §{intro} Introduction [#rationale.intro]

### B.0.1. Versioning posture pre-1.0 [#rationale.versioning-pre-1-0]
The §{intro.version} model — `MAJOR.MINOR` pre-1.0, batched cuts, convention-only conformance claim, git-tagged corpus — embeds four design decisions worth recording.

**Why `MAJOR.MINOR` not strict SemVer pre-1.0.** Breaking changes are normal pre-1.0; the discipline of distinguishing MAJOR from MINOR is performative when every cut may contain a breakage. PATCH buys nothing — there is no installed base whose conformance is broken by a typo fix that nobody has to back-port. Strict SemVer arrives at 1.0 along with an installed base for which the discipline starts to matter.

**Why version cuts MAY batch CSes.** Authoring (one CS per merge) and publication (one cut per N CSes) are different concerns. Batching keeps the cut cadence at "when an implementation needs to claim a new version" rather than "after every spec edit," which would inflate the version number into the dozens with little reader benefit.

**Why the conformance-claim mechanism is convention, not a normative requirement.** No consumer pre-1.0; §{intro.non-scope} excludes the CLI surface; no automated check enforces a MUST without an ecosystem to enforce against. The convention is documented in `CONTRIBUTING.md`, where it is amendable without a CS.

**Why the corpus is identified by git tag rather than versioned subdirectory.** Git already snapshots prose and corpus together at the tag. Versioned subdirectories duplicate that snapshot in a place where it must be manually maintained; they also imply a back-port story that pre-1.0 explicitly disclaims.
```

- [ ] **Step 3.2: Register the new slug in `standard/scripts/slug-mapping.ts`**

Find the existing `rationale.intro` entry:

```ts
  'sec-B-0':     'rationale.intro',
```

Add a new line directly below it:

```ts
  'sec-B-0':     'rationale.intro',
  'sec-B-0-1':   'rationale.versioning-pre-1-0',
```

(The `sec-B-0-1` left-hand key follows the file's existing positional convention even though we no longer use positional anchors at runtime — the registry is the project's authoritative chapter-prefix list per its file-header comment.)

- [ ] **Step 3.3: Run the site build to verify the slug and prose render**

```bash
cd standard && pnpm build && cd ..
```

Expected: build completes without errors. The `rehype-bare-ref-lint` plugin will fail the build if any `§{...}` reference points at an unregistered slug. The new B.0.1 references `§{intro.version}` and `§{intro.non-scope}`, both of which already exist.

- [ ] **Step 3.4: Commit**

```bash
git add standard/src/content/docs/appendix/B-rationale.mdx standard/scripts/slug-mapping.ts
git commit -m "$(cat <<'EOF'
spec(standard): App. B.0.1 rationale for pre-1.0 versioning posture

Fills in the B.0 placeholder with one subsection capturing four
design decisions: MAJOR.MINOR not strict SemVer, batched cuts,
convention-only conformance claims, and git-tagged corpus.

Part of CS-0012.
EOF
)"
```

---

## Task 4: Add the App. D Versions index

The first of three D-changes edits: add the top-of-page index that lists each cut and the CSes it covers.

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx` (insert Versions section after the intro blockquote)

- [ ] **Step 4.1: Insert the Versions index**

Find the existing intro block at the top of the file:

```mdx
# Appendix D. Changes (informative)

> **Informative.** This appendix is the chronological changelog of amendments to the Cook Standard. Each entry has a stable `CS-NNNN` ID, a one-line summary, the list of sections affected, and the commit / PR reference.

## CS-0001 — Cook Standard v0.1 established
```

Insert a new `## Versions` section between the blockquote and the `## CS-0001` heading:

```mdx
# Appendix D. Changes (informative)

> **Informative.** This appendix is the chronological changelog of amendments to the Cook Standard. Each entry has a stable `CS-NNNN` ID, a one-line summary, the list of sections affected, and the commit / PR reference. Entries are grouped under tagged versions; see the [Versions](#versions) index immediately below.

## Versions [#changes.versions]

- **v0.2** (`cs-standard/v0.2`, 2026-04-26) — CS-0011, CS-0012
- **v0.1** (`cs-standard/v0.1`, 2026-04-22..2026-04-24) — CS-0001 through CS-0010

## CS-0001 — Cook Standard v0.1 established
```

(The intro blockquote gains a forward reference to the new Versions section; the existing per-CS bodies below are unchanged in this task.)

- [ ] **Step 4.2: Register the new slug in `standard/scripts/slug-mapping.ts`**

Find the chapter-D block in the slug mapping (search for `changes.cs-0001` or similar). Add a new entry alongside the chapter-D entries:

```ts
  // Versions index (CS-0012)
  'sec-D-versions': 'changes.versions',
```

(If a `// ── Appendix D ──` block doesn't already exist, place the new entry at the end of the file, before the closing brace of `SLUG_MAPPING`, with a clear comment.)

- [ ] **Step 4.3: Run the site build**

```bash
cd standard && pnpm build && cd ..
```

Expected: build succeeds. Inspect `dist/appendix/d-changes/index.html` and confirm the Versions section renders before the CS-0001 heading and shows both bullets.

- [ ] **Step 4.4: Commit**

```bash
git add standard/src/content/docs/appendix/D-changes.mdx standard/scripts/slug-mapping.ts
git commit -m "$(cat <<'EOF'
spec(standard): add App. D Versions index

New top-of-appendix section grouping CSes under tagged versions.
v0.1 covers CS-0001..CS-0010; v0.2 covers CS-0011, CS-0012.

Part of CS-0012.
EOF
)"
```

---

## Task 5: Backfill `**Version:**` lines in CS-0001..CS-0011 bodies

Mechanical sweep: every existing CS body gains a `**Version:**` line between `**Date**` and `**Sections affected**`.

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx` (eleven per-CS edits)

- [ ] **Step 5.1: Add `**Version:** v0.1` to CS-0001..CS-0010**

For each of CS-0001, CS-0002, CS-0003, CS-0004, CS-0005, CS-0006, CS-0007, CS-0008, CS-0009, CS-0010 in `D-changes.mdx`, locate the `**Date:** ...` line in that CS's body and insert directly after it:

```mdx
**Version:** v0.1
```

Spot-check shape — CS-0001 should now read:

```mdx
## CS-0001 — Cook Standard v0.1 established

**Date:** 2026-04-22
**Version:** v0.1
**Sections affected:** entire Standard (establishment).
```

CS-0002 ("Planned: tree-sitter-cook conformance audit") gets `**Version:** v0.1` because its entry was authored in the v0.1 era, even though the tracked work itself is unscheduled. Note that this back-tags the *entry's authorship*, not the planned future work.

- [ ] **Step 5.2: Add `**Version:** v0.2` to CS-0011**

Same edit pattern for CS-0011: insert `**Version:** v0.2` after its `**Date:**` line.

- [ ] **Step 5.3: Verify the eleven insertions**

```bash
grep -c '^\*\*Version:\*\*' standard/src/content/docs/appendix/D-changes.mdx
```

Expected output: `11`.

```bash
grep -c '^\*\*Version:\*\* v0\.1$' standard/src/content/docs/appendix/D-changes.mdx
```

Expected output: `10`.

```bash
grep -c '^\*\*Version:\*\* v0\.2$' standard/src/content/docs/appendix/D-changes.mdx
```

Expected output: `1`.

- [ ] **Step 5.4: Run the site build**

```bash
cd standard && pnpm build && cd ..
```

Expected: build succeeds.

- [ ] **Step 5.5: Commit**

```bash
git add standard/src/content/docs/appendix/D-changes.mdx
git commit -m "$(cat <<'EOF'
spec(standard): backfill **Version:** lines in CS-0001..CS-0011

CS-0001..CS-0010 marked v0.1 (covers entries authored before the
versioning machinery landed). CS-0011 marked v0.2.

Part of CS-0012.
EOF
)"
```

---

## Task 6: Add the CS-0012 entry to App. D

Now that everything CS-0012 references exists, write its D-changes entry.

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx` (append CS-0012 entry at the bottom)

- [ ] **Step 6.1: Append the CS-0012 entry**

At the end of the file, after the existing CS-0011 entry, append:

```mdx

## D.12. CS-0012 — Cook Standard versioning, App. D reshape, conformance-claim convention. [#changes.cs-0012]

**Date:** 2026-04-26
**Version:** v0.2
**Sections affected:** §{intro.version} (rewritten); §{intro.conformance} (amended); App. B.0.1 (new); App. D (new Versions index, per-CS `**Version:**` lines, this entry); `standard/VERSION` (new); `standard/src/content/docs/index.mdx` (Status header now reads from VERSION); `standard/scripts/slug-mapping.ts` (new slug `rationale.versioning-pre-1-0`, new slug `changes.versions`); `standard/README.md` (cross-link to the cut procedure); `CONTRIBUTING.md` (two new subsections: cut procedure and implementation conformance claims).

**Summary:** Adopts `MAJOR.MINOR` versioning for the pre-1.0 era. A *cut* (= MINOR bump + App. D Versions index entry + `cs-standard/vX.Y` git tag, all in one commit on `main`) MAY batch one or more CS entries; CSes per cut and version cadence are independent. No PATCH track pre-1.0. The transition to strict SemVer at 1.0 is deferred.

`standard/VERSION` becomes the canonical machine-readable version source; the docs index `index.mdx` imports it via Vite's `?raw` and renders `v{VERSION}` in the page header. §{intro.conformance} is amended to define what "conforms to Cook Standard v0.X" means (the implementation satisfies the existing three points against the prose and corpus of the `cs-standard/v0.X` tag); the mechanism by which an implementation claims a version is implementation-defined and not normatively required.

App. D is restructured: a top-of-page Versions index lists each cut and the CSes it covers; every CS body now carries a `**Version:**` line between `**Date**` and `**Sections affected**`. CS-0001 through CS-0010 are grouped under v0.1; CS-0011 and CS-0012 are grouped under v0.2.

App. B.0.1 (new) records the rationale for `MAJOR.MINOR` over strict SemVer pre-1.0, batched cuts, convention-only conformance claims, and git-tagged corpus. `CONTRIBUTING.md` gains the procedural recipe for cutting a version and the project convention by which an implementation claims a Standard version.

**Implementation status.** This change is authoring-surface only. No conformance-corpus changes. Two `cs-standard/vX.Y` tags are cut as a manual post-merge step (see the design's §4 for the exact `git merge-base` and `git tag` invocations).

**Reference:** this commit.
```

- [ ] **Step 6.2: Verify the entry references resolve**

```bash
cd standard && pnpm build && cd ..
```

Expected: build succeeds. The `rehype-bare-ref-lint` plugin will catch any broken `§{...}` reference; the entry references `§{intro.version}`, `§{intro.conformance}` (both pre-existing). If new slugs introduced in earlier tasks (`rationale.versioning-pre-1-0`, `changes.versions`) are referenced and fail to resolve, double-check Task 3.2 and Task 4.2 ran cleanly.

- [ ] **Step 6.3: Commit**

```bash
git add standard/src/content/docs/appendix/D-changes.mdx
git commit -m "$(cat <<'EOF'
spec(standard): add CS-0012 D-changes entry

Records the versioning machinery, App. D reshape, and conformance-claim
convention introduced by this CS.

Part of CS-0012.
EOF
)"
```

---

## Task 7: Update `CONTRIBUTING.md` with cut procedure and claim convention

The convention layer that the Standard intentionally does not normatively require.

**Files:**
- Modify: `CONTRIBUTING.md` (two new subsections, inserted after the existing "Conformance" subsection)

- [ ] **Step 7.1: Add the "Cutting a Cook Standard version" subsection**

Find the existing `### Conformance` block (currently lines 40-43 of `CONTRIBUTING.md`). Directly after the line `- A tree-sitter harness against the same corpus is planned; see \`D-changes.mdx\` CS-0002.`, insert:

```markdown

### Cutting a Cook Standard version

The Standard uses `MAJOR.MINOR` versioning pre-1.0 (see [`§ 0.5`](standard/src/content/docs/00-introduction.mdx)). A *cut* publishes a new MINOR by performing three actions in a single commit on `main`:

1. Bump `standard/VERSION` to the next MINOR (e.g. `0.2` → `0.3`).
2. Add a new entry to the top of the App. D **Versions** index in `standard/src/content/docs/appendix/D-changes.mdx`, listing the CSes the cut covers.
3. Set each batched CS body's `**Version:**` line to the new version.

After the commit lands on `main`, tag it:

```bash
git tag cs-standard/vX.Y
git push origin cs-standard/vX.Y
```

The tag and the index entry together constitute the published cut.

The cut commit may also batch the CS being cut (i.e. the CS that introduced the cut-worthy change can perform the bump in the same commit). There is no rule against a cut containing exactly one CS — it is simply not required.
```

- [ ] **Step 7.2: Add the "Implementation conformance claims" subsection**

Directly after the "Cutting a Cook Standard version" subsection from Step 7.1, insert:

```markdown

### Implementation conformance claims

The Cook Standard does not normatively require an implementation to expose its claimed Standard version (see [`§ 0.7`](standard/src/content/docs/00-introduction.mdx)). As a project convention:

- **`cli/crates/cook-lang`** — set a `pub const COOK_STANDARD_VERSION: &str = "X.Y";` in the crate root, mirrored into the README badge or status line.
- **`tree-sitter-cook`** (when CS-0002 lands) — set the claimed version in a header comment in `grammar.js`.
- **Each implementation's README** — state the claimed version in the project description.

These are not enforced by any automated check pre-1.0; they are a project discipline. When the Standard cuts a new version, each implementation is responsible for either updating its claim or accepting that it now lags the Standard by one version.
```

- [ ] **Step 7.3: Verify the file still renders cleanly as Markdown**

```bash
head -90 CONTRIBUTING.md | tail -50
```

Expected: the new "Cutting a Cook Standard version" and "Implementation conformance claims" subsections are present, well-formed, and follow the existing `### `-level structure of the file.

- [ ] **Step 7.4: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "$(cat <<'EOF'
docs(contributing): cut procedure + implementation conformance claims

Documents the three-action cut recipe (VERSION bump + App. D Versions
index entry + cs-standard/vX.Y tag) and the convention by which each
implementation states its claimed Standard version.

Part of CS-0012.
EOF
)"
```

---

## Task 8: Cross-link from `standard/README.md`

Make spec maintainers find the cut procedure from inside the standard directory.

**Files:**
- Modify: `standard/README.md` (one link addition near the existing CONTRIBUTING reference on line 41)

- [ ] **Step 8.1: Add a sibling reference to the cut procedure**

Find the existing line in `standard/README.md`:

```markdown
See `../CONTRIBUTING.md` for the spec-first rule. A change to a Cookfile surface construct must update `src/content/docs/` in the same commit as the implementation change, and must add a `CS-NNNN` entry to `src/content/docs/appendix/D-changes.mdx`.
```

Add a follow-on sentence at the end of that paragraph:

```markdown
See `../CONTRIBUTING.md` for the spec-first rule. A change to a Cookfile surface construct must update `src/content/docs/` in the same commit as the implementation change, and must add a `CS-NNNN` entry to `src/content/docs/appendix/D-changes.mdx`. To publish a new MINOR version of the Standard, see the **Cutting a Cook Standard version** subsection in the same file.
```

- [ ] **Step 8.2: Commit**

```bash
git add standard/README.md
git commit -m "$(cat <<'EOF'
docs(standard): link standard/README.md to cut procedure

Spec maintainers working inside standard/ should find the version-cut
recipe from the in-directory README.

Part of CS-0012.
EOF
)"
```

---

## Task 9: Final verification

Run the full verification surface: site build, keyword lint, conformance harness.

**Files:** none modified.

- [ ] **Step 9.1: Run the full standard build**

```bash
cd standard && pnpm build && cd ..
```

Expected: build completes cleanly. Confirms all `§{...}` refs resolve and the rendered Status line in `dist/index.html` shows `v0.2`.

- [ ] **Step 9.2: Run the normative-keyword lint**

```bash
cd standard && pnpm lint:keywords && cd ..
```

Expected: no findings, or only pre-existing findings unrelated to this CS.

- [ ] **Step 9.3: Run the conformance harness**

```bash
cargo test -p cook-lang --test conformance
```

Expected: harness passes. CS-0012 is authoring-surface only and changes no corpus, so behaviour is identical to the pre-CS-0012 baseline. If this fails, surface the failure — it indicates a separate regression, not a CS-0012 problem.

- [ ] **Step 9.4: Confirm the Versions index renders correctly**

```bash
grep -A6 '## Versions' standard/dist/appendix/d-changes/index.html | head -20
```

Expected: HTML shows the Versions h2 with both `v0.2` and `v0.1` bullets and the CS lists.

- [ ] **Step 9.5: Confirm `standard/VERSION` is the only version source**

```bash
grep -rn '"0\.2"\|v0\.2' standard/src/ standard/scripts/ 2>/dev/null | grep -v 'd-changes.mdx\|00-introduction.mdx\|index.mdx'
```

Expected: empty output. Any hit indicates a stray hard-coded version that should reference `standard/VERSION` instead — fix it before declaring the CS done.

- [ ] **Step 9.6: Confirm the new commits are logically grouped**

```bash
git log --oneline main..HEAD | head -20
```

Expected: a clean sequence of `spec(standard): ...` and `docs(...): ...` commits, all marked "Part of CS-0012," with the CS-0011 commits preceding them on the branch.

---

## Post-merge actions (manual, NOT executed by this plan)

These run once, after CS-0011 + CS-0012 merge to `main`. The plan documents the exact commands; do not run them as part of the implementation.

- [ ] **Step P.1: Tag `cs-standard/v0.1`**

```bash
git fetch origin
v01_commit=$(git merge-base origin/main feat/cs-0011-remove-vardecl)
git tag cs-standard/v0.1 "$v01_commit"
```

`git merge-base` resolves to the commit on `main` immediately before the CS-0011 branch diverged. Verify the resolved commit is the one you expect (it should be the most recent pre-CS-0011 `main` commit) before tagging.

- [ ] **Step P.2: Tag `cs-standard/v0.2`**

```bash
git tag cs-standard/v0.2 origin/main
```

After the CS-0012 commits merge to `main`, this tags the merge commit (or the tip of `main` if the merge fast-forwarded).

- [ ] **Step P.3: Push both tags**

```bash
git push origin cs-standard/v0.1 cs-standard/v0.2
```

- [ ] **Step P.4: Update `cli/crates/cook-lang` to claim v0.2**

Per the convention in `CONTRIBUTING.md` "Implementation conformance claims," add to `cli/crates/cook-lang/src/lib.rs` (or the crate root, wherever appropriate):

```rust
/// The Cook Standard version this crate claims conformance to.
/// See standard/src/content/docs/appendix/D-changes.mdx for the
/// changelog of that version.
pub const COOK_STANDARD_VERSION: &str = "0.2";
```

Commit this on `main` (or as a follow-up) with a `feat(cook-lang): claim Cook Standard v0.2 conformance` message. This is **not** part of CS-0012 itself — it is the first use of the convention CS-0012 establishes.
