# Cook Standard v0.10 Structural Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reorganise the Cook Standard at v0.10 into four explicit Parts with per-topic chapters per the approved design at `standard/specs/2026-05-13-standard-reorg-design.md`.

**Architecture:** Single PR composed of nine logical commits. New MDX files are added alongside the old, then the old files are deleted in a dedicated commit so `git mv`-style blame is preserved where content survives intact. Slug renames flow through a new `scripts/slug-renames.ts` registry that drives both build-emitted redirects and a build-time lint for retired slugs in source. ~160 lines of new normative prose (§24 Both-phase API, §12 Modules lifecycle) land in the same cut. No Cookfile-language behaviour changes; the conformance corpus is unchanged.

**Tech Stack:** TypeScript (Astro/Starlight, Vitest, remark/rehype plugins), MDX content, pnpm.

**Worktree:** Create a worktree before starting:
```bash
cd /home/alex/dev/cook
git worktree add ../cook-standard-reorg -b standard-v0.10-reorg main
cd ../cook-standard-reorg
```

**Working directory for every shell step below:** `/home/alex/dev/cook-standard-reorg/standard/` (the worktree's `standard/` subdirectory). Treat every path that begins with `src/`, `scripts/`, `astro.config.mjs`, or `package.json` as relative to that directory.

---

## Reference A — Slug rename table

This is the canonical map. Tasks 3–7 rewrite every `§{old}` reference in source to `§{new}` and every `[#old]` heading marker to `[#new]`. Task 1 codifies this same table in `scripts/slug-renames.ts`.

### A.1. Retired chapter slugs

| Old slug | New slug | Reason |
|---|---|---|
| `grammar` | (chapter retired; sub-slugs distribute below) | Ch. 3 splits across Ch. 4, 5, 8 |
| `grammar.overview` | `toplevel.overview` | Top-level production overview moves to Ch. 4 |
| `grammar.top-level-ordering` | `toplevel.ordering` | Top-level ordering rule moves to Ch. 4 |
| `grammar.var-declarations` | *(no replacement — deleted in CS-0011)* | Already removed from grammar; entry retired |
| `grammar.use-declarations` | `decl.use` | Ch. 5 |
| `grammar.import-declarations` | `decl.import` | Ch. 5 |
| `grammar.config-blocks` | `decl.config` | Ch. 5 |
| `grammar.config-composition` | `decl.config-composition` | Ch. 5 |
| `grammar.register-blocks` | `decl.register` | Ch. 5 |
| `grammar.register-blocks.splicing` | `decl.register-splicing` | Ch. 5 |
| `grammar.recipe-syntax` | `recipes.header-forms` | Ch. 6 (merge with existing) |
| `grammar.step-dispatch` | `steps.dispatch` | Ch. 8 |
| `grammar.top-level-module-call` | `toplevel.module-call` | Ch. 4 |
| `modules` | (chapter retired; sub-slugs distribute below) | Ch. 7 splits across Ch. 11 and Ch. 12 |
| `modules.overview` | `comp.overview` | Ch. 11 |
| `modules.import-declaration` | `comp.import` | Ch. 11 |
| `modules.qualified-refs` | `comp.qualified-refs` | Ch. 11 |
| `modules.use-scope` | `mods.use-scope` | Ch. 12 (`use`-system topic) |
| `modules.duplicates-and-cycles` | `comp.duplicates-and-cycles` | Ch. 11 |
| `modules.workspace-root` | `exec.ws` | Ch. 20 (workspace root is invocation-time) |
| `modules.cache-invariants` | `exec.cache.portability` | Ch. 17 |
| `stdmods` | (chapter retired; sub-slugs distribute below) | Ch. 9 splits across Ch. 27 and Ch. 28 |
| `stdmods.bootstrap` | `cat.bootstrap` | Ch. 27 |
| `stdmods.bootstrap.install` | `cat.bootstrap.install` | Ch. 27 |
| `stdmods.bootstrap.vendor` | `cat.bootstrap.vendor` | Ch. 27 |
| `stdmods.bootstrap.catalogue` | `cat.index` | Ch. 27 |
| `stdmods.cc.*` | `cat.cc.*` | Ch. 28, leaves preserved |
| `lexical.placeholders` | `phl.token` | §2.11 moves to Ch. 9 |

### A.2. Preserved slug prefixes (no rewriting needed)

`intro.*`, `notation.*`, `lexical.*` (except `lexical.placeholders`), `recipes.*`, `chores.*`, `xref.*`, `lua.*` (every leaf), `exec.*` (every existing leaf — new leaves added below), `grammar-appendix.*`, `rationale.*`, `examples.*`, `changes.*`.

### A.3. New slug prefixes introduced

`conf.*`, `toplevel.*`, `decl.*`, `steps.*`, `phl.*`, `comp.*`, `mods.*`, `cat.*`, `pre-v1.*`, `corpus.*`.

---

## Reference B — Per-chapter content migration map

What moves from which source location into each new chapter.

| New file | Source file(s) | Content |
|---|---|---|
| `00-introduction.mdx` | current `00-introduction.mdx` | Keep §0.1–§0.5 verbatim. §0.6 retained until a separate editorial pass removes it. §0.7 moves out. |
| `01-conformance.mdx` | current `00-introduction.mdx` §0.7 | Whole section becomes Ch. 1; expand the lead-in paragraph. |
| `02-notation.mdx` | current `01-notation.mdx` | Verbatim. |
| `03-lexical.mdx` | current `02-lexical.mdx` | Everything except §2.11. |
| `04-toplevel.mdx` | current `03-syntactic-grammar.mdx` §3.1, §3.2, §3.7.5; current `04-recipes.mdx` §4.1.1 | Top-level production, ordering, body-termination rule, top-level module_call. |
| `05-declarations.mdx` | current `03-syntactic-grammar.mdx` §3.4–§3.7 | `use`, `import`, `config` (+composition), `register` (+splicing). |
| `06-recipes.mdx` | current `03-syntactic-grammar.mdx` §3.8; current `04-recipes.mdx` §4.1, §4.2; current `04-recipes.mdx` Note 4.4.2 | Recipe header, dep list, body shape; promote Note 4.4.2 region rule to a numbered subsection. |
| `07-chores.mdx` | current `04a-chores.mdx` | Verbatim (renamed slug; promoted to real chapter). |
| `08-step-kinds.mdx` | current `03-syntactic-grammar.mdx` §3.9; current `04-recipes.mdx` §4.3, §4.4 (overview table), §4.5–§4.10 | One normative home for step dispatch + every step body. |
| `09-placeholders.mdx` | current `02-lexical.mdx` §2.11; current `06-cook-lua-api.mdx` §6.7, §6.7.1 | Token shape, resolution cross-ref, cook-step tables, plate/test tables. |
| `10-cross-recipe-references.mdx` | current `05-cross-recipe-references.mdx` §5.1–§5.7 | Verbatim; cross-refs to §9 for placeholder tables. |
| `11-cross-cookfile-composition.mdx` | current `07-cross-cookfile-composition.mdx` §7.1–§7.5 | Loses §7.6 and §7.7 (move to Part II). |
| `12-modules.mdx` | current `06-cook-lua-api.mdx` §6.8; current `09-standard-modules.mdx` §9.1; new lifecycle prose | `use` declaration, resolution, lifecycle, catalogue index pointer. |
| `13-two-phase.mdx` | current `08-execution-model.mdx` §8.1, §8.1.2 | Two-phase + phase classification table. |
| `14-capture-mode.mdx` | current `08-execution-model.mdx` §8.2 | Capture mode. |
| `15-step-groups.mdx` | current `08-execution-model.mdx` §8.3; current `04-recipes.mdx` §4.9.3 | Step groups + body bundling. |
| `16-ordering-drain.mdx` | current `08-execution-model.mdx` §8.4, §8.4.1, §8.5 | Cross-recipe ordering, output-path uniqueness, interactive drain. |
| `17-cache.mdx` | current `08-execution-model.mdx` §8.6–§8.6.4; current `07-cross-cookfile-composition.mdx` §7.7 | Cache semantics + integrity + discovered-inputs + test-unit + portability invariants. |
| `18-output-materialisation.mdx` | current `08-execution-model.mdx` §8.7 | Verbatim. |
| `19-diagnostics.mdx` | current `08-execution-model.mdx` §8.8 | Verbatim. |
| `20-workspace.mdx` | current `07-cross-cookfile-composition.mdx` §7.6 | Workspace root determination. |
| `21-lua-api.mdx` | current `06-cook-lua-api.mdx` §6.1, §6.1.1, §6.1.2 | API surface overview (chapter intro). |
| `22-register-phase.mdx` | current `06-cook-lua-api.mdx` §6.2, §6.2.1, §6.3.2, §6.3.3, §6.3.5 | `cook.add_unit`, `cook.exec`/`cook.interactive`, `cook.recipe`, `cook.add_test`. Reference §17 for `discovered_inputs` semantics. |
| `23-execute-phase.mdx` | current `06-cook-lua-api.mdx` §6.4, §6.4.1 | Using-block globals + plate/test Lua bindings. |
| `24-both-phase.mdx` | current `06-cook-lua-api.mdx` §6.3.1, §6.3.4; new normative prose | `cook.sh`, `cook.load_module`, and new sections for `cook.env`, `cook.cache`, `cook.export`/`cook.import`, `cook.platform`, `cook.dep_output`/`cook.dep_output_list`. |
| `25-fs.mdx` | current `06-cook-lua-api.mdx` §6.5, §6.5.8, §6.5.9 | `fs.*` + sandbox + shell escape hatches. |
| `26-path.mdx` | current `06-cook-lua-api.mdx` §6.6 | `path.*` helpers. |
| `27-catalogue.mdx` | current `09-standard-modules.mdx` §9.1 | Catalogue governance + index. |
| `28-cc.mdx` | current `09-standard-modules.mdx` §9.2 | `cc` module — its own file. |
| `appendix/A-grammar.mdx` | current `appendix/A-grammar.mdx` | Verbatim. |
| `appendix/B-examples.mdx` | current `appendix/C-examples.mdx` | Renamed from C → B. |
| `appendix/C-rationale.mdx` | current `appendix/B-rationale.mdx` | Renamed from B → C; heading numbers renumber to match new chapters; slugs preserved. |
| `appendix/D-pre-v1-checklist.mdx` | current `appendix/E-pre-v1-checklist.mdx` | Renamed from E → D; assign `pre-v1.*` slugs. |
| `appendix/E-changes.mdx` | current `appendix/D-changes.mdx` | Renamed from D → E; restructure with per-version subheaders. |
| `appendix/F-corpus.mdx` | (new stub) | Placeholder for future corpus embedding. |

---

## Reference C — Standard task header

Each content-migration task uses this template for its sub-steps:

1. **Create the new file** with Astro frontmatter (`title`, `sidebar.order`).
2. **Insert a Normative or Informative banner** matching the new chapter's status.
3. **Copy source material** from the listed source section(s).
4. **Rewrite slug references in the moved material** per Reference A.
5. **Rewrite heading numbers** to match the new chapter (top-level heading is `# N. Title [#slug]`; sub-sections are `## N.M`, `### N.M.K`, etc.; sub-sections four levels deep MUST NOT appear per §{notation.numbering-and-citation}).
6. **Update the `[#slug]` marker on every heading** to the new slug.
7. **Build the site** (`pnpm build`) and visit the new page to verify rendering.
8. **Commit** with `git add` of the new file only (deletion of source files happens in Task 8).

The build will emit slug-rename warnings during Tasks 3–7. These warnings are expected; they convert to hard errors in Task 8.

---

## Task 1: Pre-flight infrastructure

**Files:**
- Create: `scripts/slug-renames.ts`
- Modify: `scripts/slug-mapping.ts` (add new slug stubs)
- Modify: `src/plugins/remark-slug-xrefs.ts` (consult rename registry)
- Modify: `astro.config.mjs` (wire redirects from `slug-renames.ts`)
- Create: `src/plugins/__tests__/slug-renames-consistency.test.ts`
- Create: `src/plugins/__tests__/no-retired-slugs-in-source.test.ts`

- [ ] **Step 1.1: Read existing slug infra**

Read `scripts/slug-mapping.ts` and `src/plugins/remark-slug-xrefs.ts` to confirm shape before editing.

```bash
wc -l scripts/slug-mapping.ts src/plugins/remark-slug-xrefs.ts
```

Expected: ~199 lines for slug-mapping.ts, ~51 lines for remark-slug-xrefs.ts.

- [ ] **Step 1.2: Create the slug-renames registry**

Create `scripts/slug-renames.ts` with the full retired→new map from Reference A.1:

```ts
// Slug renames for the Cook Standard v0.10 structural redesign.
//
// One-way map from a retired slug to its replacement. The Astro build
// reads this map to emit redirects from old anchored URLs to new ones,
// and the remark-slug-xrefs plugin consults it on a missing slug to
// emit a precise rename error.
//
// Keep entries in retired-slug alphabetical order.

export const SLUG_RENAMES: Record<string, string | null> = {
  // null means: retired with no replacement (already removed from the
  // language by a prior CS entry).

  'grammar':                         'toplevel.overview',
  'grammar.overview':                'toplevel.overview',
  'grammar.top-level-ordering':      'toplevel.ordering',
  'grammar.var-declarations':        null,
  'grammar.use-declarations':        'decl.use',
  'grammar.import-declarations':     'decl.import',
  'grammar.config-blocks':           'decl.config',
  'grammar.config-composition':      'decl.config-composition',
  'grammar.register-blocks':         'decl.register',
  'grammar.register-blocks.splicing':'decl.register-splicing',
  'grammar.recipe-syntax':           'recipes.header-forms',
  'grammar.step-dispatch':           'steps.dispatch',
  'grammar.top-level-module-call':   'toplevel.module-call',

  'modules':                         'comp.overview',
  'modules.overview':                'comp.overview',
  'modules.import-declaration':      'comp.import',
  'modules.qualified-refs':          'comp.qualified-refs',
  'modules.use-scope':               'mods.use-scope',
  'modules.duplicates-and-cycles':   'comp.duplicates-and-cycles',
  'modules.workspace-root':          'exec.ws.determination',
  'modules.cache-invariants':        'exec.cache.portability',

  'stdmods':                         'cat.bootstrap',
  'stdmods.bootstrap':               'cat.bootstrap',
  'stdmods.bootstrap.install':       'cat.bootstrap.install',
  'stdmods.bootstrap.vendor':        'cat.bootstrap.vendor',
  'stdmods.bootstrap.catalogue':     'cat.index',
  'stdmods.cc':                      'cat.cc',
  'stdmods.cc.synopsis':             'cat.cc.synopsis',
  'stdmods.cc.identity':             'cat.cc.identity',
  'stdmods.cc.surface':              'cat.cc.surface',
  'stdmods.cc.bin':                  'cat.cc.bin',
  'stdmods.cc.lib':                  'cat.cc.lib',
  'stdmods.cc.shared':               'cat.cc.shared',
  'stdmods.cc.headers':              'cat.cc.headers',
  'stdmods.cc.compile':              'cat.cc.compile',
  'stdmods.cc.archive':              'cat.cc.archive',
  'stdmods.cc.link':                 'cat.cc.link',
  'stdmods.cc.find':                 'cat.cc.find',
  'stdmods.cc.find-cmake-compat':    'cat.cc.find-cmake-compat',
  'stdmods.cc.find-cmake-compile':   'cat.cc.find-cmake-compile',
  'stdmods.cc.find-cmake-link':      'cat.cc.find-cmake-link',
  'stdmods.cc.defaults':             'cat.cc.defaults',
  'stdmods.cc.toolchain':            'cat.cc.toolchain',
  'stdmods.cc.compile-commands':     'cat.cc.compile-commands',
  'stdmods.cc.register-finder':      'cat.cc.register-finder',
  'stdmods.cc.find-or-error':        'cat.cc.find-or-error',
  'stdmods.cc.transitive':           'cat.cc.transitive',
  'stdmods.cc.errors':               'cat.cc.errors',
  'stdmods.cc.vendoring':            'cat.cc.vendoring',

  'lexical.placeholders':            'phl.token',
};

export function resolveRename(retired: string): string | null | undefined {
  return Object.prototype.hasOwnProperty.call(SLUG_RENAMES, retired)
    ? SLUG_RENAMES[retired]
    : undefined;
}
```

- [ ] **Step 1.3: Add new slug stubs to `slug-mapping.ts`**

Append the new slugs at the end of `SLUG_MAPPING` so the consistency test in Step 1.5 can verify replacement targets exist. Section numbers will be re-finalised in Task 8.

Open `scripts/slug-mapping.ts` and add this block just before the closing `};` (line 199):

```ts
  // ── v0.10 reorg: new slug prefixes ────────────────────────────────────────
  // These are stubs that the v0.10 cut populates with real section numbers
  // (the keys remain the positional `sec-N-M-K` form once new chapters are
  // numbered).

  // Ch. 1 — Conformance
  'sec-1-new':           'conf',
  'sec-1-new-1':         'conf.criteria',

  // Ch. 4 — Top-level structure
  'sec-4-new':           'toplevel',
  'sec-4-new-1':         'toplevel.overview',
  'sec-4-new-2':         'toplevel.ordering',
  'sec-4-new-3':         'toplevel.termination',
  'sec-4-new-4':         'toplevel.module-call',

  // Ch. 5 — Declarations
  'sec-5-new':           'decl',
  'sec-5-new-1':         'decl.use',
  'sec-5-new-2':         'decl.import',
  'sec-5-new-3':         'decl.config',
  'sec-5-new-3-1':       'decl.config-composition',
  'sec-5-new-4':         'decl.register',
  'sec-5-new-4-1':       'decl.register-splicing',

  // Ch. 8 — Step kinds
  'sec-8-new':           'steps',
  'sec-8-new-1':         'steps.dispatch',
  'sec-8-new-2':         'steps.ingredients',
  'sec-8-new-3':         'steps.cook-single',
  'sec-8-new-4':         'steps.cook-multi',
  'sec-8-new-5':         'steps.iteration-mode',
  'sec-8-new-6':         'steps.plate',
  'sec-8-new-7':         'steps.test',
  'sec-8-new-8':         'steps.lua',
  'sec-8-new-9':         'steps.shell',

  // Ch. 9 — Placeholders
  'sec-9-new':           'phl',
  'sec-9-new-1':         'phl.token',
  'sec-9-new-2':         'phl.resolution',
  'sec-9-new-3':         'phl.cook-step',
  'sec-9-new-4':         'phl.plate-test',

  // Ch. 11 — Cross-Cookfile composition
  'sec-11-new':          'comp',
  'sec-11-new-1':        'comp.overview',
  'sec-11-new-2':        'comp.import',
  'sec-11-new-3':        'comp.qualified-refs',
  'sec-11-new-4':        'comp.duplicates-and-cycles',

  // Ch. 12 — Modules (use system + catalogue index)
  'sec-12-new':          'mods',
  'sec-12-new-1':        'mods.use',
  'sec-12-new-2':        'mods.use-scope',
  'sec-12-new-3':        'mods.lifecycle',
  'sec-12-new-4':        'mods.builtin',
  'sec-12-new-5':        'mods.local',
  'sec-12-new-6':        'mods.catalogue-index',

  // Ch. 20 — Workspace root (Part II)
  'sec-20-new':          'exec.ws',
  'sec-20-new-1':        'exec.ws.determination',
  'sec-17-new-portability': 'exec.cache.portability',

  // Ch. 27 — Catalogue governance
  'sec-27-new':          'cat',
  'sec-27-new-1':        'cat.index',
  'sec-27-new-2':        'cat.bootstrap',
  'sec-27-new-2-1':      'cat.bootstrap.install',
  'sec-27-new-2-2':      'cat.bootstrap.vendor',

  // Ch. 28 — cc module
  'sec-28-new':          'cat.cc',
  'sec-28-new-1':        'cat.cc.synopsis',
  'sec-28-new-2':        'cat.cc.identity',
  'sec-28-new-3':        'cat.cc.surface',
  'sec-28-new-3-1':      'cat.cc.bin',
  'sec-28-new-3-2':      'cat.cc.lib',
  'sec-28-new-3-3':      'cat.cc.shared',
  'sec-28-new-3-4':      'cat.cc.headers',
  'sec-28-new-3-5':      'cat.cc.compile',
  'sec-28-new-3-6':      'cat.cc.archive',
  'sec-28-new-3-7':      'cat.cc.link',
  'sec-28-new-3-8':      'cat.cc.find',
  'sec-28-new-3-8-1':    'cat.cc.find-cmake-compat',
  'sec-28-new-3-8-2':    'cat.cc.find-cmake-compile',
  'sec-28-new-3-8-3':    'cat.cc.find-cmake-link',
  'sec-28-new-3-9':      'cat.cc.defaults',
  'sec-28-new-3-10':     'cat.cc.toolchain',
  'sec-28-new-3-11':     'cat.cc.compile-commands',
  'sec-28-new-3-12':     'cat.cc.register-finder',
  'sec-28-new-3-13':     'cat.cc.find-or-error',
  'sec-28-new-4':        'cat.cc.transitive',
  'sec-28-new-5':        'cat.cc.errors',
  'sec-28-new-6':        'cat.cc.vendoring',

  // Annex D (was E) — Pre-1.0 checklist
  'sec-D-pre-v1':        'pre-v1',
  'sec-D-pre-v1-1':      'pre-v1.parse-txt-coupling',
  'sec-D-pre-v1-2':      'pre-v1.template-vs-bash-expansion',
  'sec-D-pre-v1-3':      'pre-v1.no-string-escape',

  // Annex F — Conformance corpus stub
  'sec-F-corpus':        'corpus',
```

(The `pre-v1` sub-slugs follow the existing checklist's section numbering; verify against `appendix/E-pre-v1-checklist.mdx` and add additional entries if the checklist has more numbered sections.)

- [ ] **Step 1.4: Write the consistency test**

Create `src/plugins/__tests__/slug-renames-consistency.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { SLUG_RENAMES } from '../../../scripts/slug-renames.ts';
import { SLUG_MAPPING } from '../../../scripts/slug-mapping.ts';

const livingSlugs = new Set(Object.values(SLUG_MAPPING));

describe('slug-renames registry', () => {
  it('every retired slug names a replacement that exists in slug-mapping (or null)', () => {
    const missing: string[] = [];
    for (const [retired, replacement] of Object.entries(SLUG_RENAMES)) {
      if (replacement === null) continue;
      if (!livingSlugs.has(replacement)) {
        missing.push(`${retired} -> ${replacement}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it('no retired slug is itself a living slug', () => {
    const collisions: string[] = [];
    for (const retired of Object.keys(SLUG_RENAMES)) {
      if (livingSlugs.has(retired)) {
        collisions.push(retired);
      }
    }
    expect(collisions).toEqual([]);
  });
});
```

- [ ] **Step 1.5: Run the consistency test**

```bash
pnpm test src/plugins/__tests__/slug-renames-consistency.test.ts
```

Expected: both tests PASS. If the first fails, a replacement slug in `slug-renames.ts` doesn't yet exist in `slug-mapping.ts` — go back to Step 1.3. If the second fails, a retired slug is still present in `slug-mapping.ts` from before — leave it; this is expected during Tasks 3–7 (the existing entries for the retired slugs stay in `slug-mapping.ts` until Task 8 removes them). Adjust the test to skip this check until Task 8:

```ts
  // Skipped until Task 8 finalises slug-mapping.ts by removing retired entries.
  it.skip('no retired slug is itself a living slug', () => { /* ... */ });
```

Re-run the test. Expected: first test PASS, second test SKIP.

- [ ] **Step 1.6: Write the source-lint test (skipped initially)**

Create `src/plugins/__tests__/no-retired-slugs-in-source.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { SLUG_RENAMES } from '../../../scripts/slug-renames.ts';

function walkMdx(dir: string): string[] {
  const out: string[] = [];
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) out.push(...walkMdx(p));
    else if (ent.isFile() && p.endsWith('.mdx')) out.push(p);
  }
  return out;
}

const CONTENT_ROOT = path.resolve(__dirname, '../../../src/content/docs');
const retired = Object.keys(SLUG_RENAMES);
const REF_RE = /§\{([a-z][a-z0-9.\-]*)\}/g;
const ANCHOR_RE = /\[#([a-z][a-z0-9.\-]*)\]/g;

describe('source files contain no retired slugs', () => {
  // Tasks 3–7 leave retired refs in untouched source files. Task 8 deletes
  // those files and flips this test to active. Until then, this test is
  // skipped to avoid blocking intermediate commits.
  it.skip('no §{retired-slug} in any rendered source', () => {
    const offences: string[] = [];
    for (const file of walkMdx(CONTENT_ROOT)) {
      const text = fs.readFileSync(file, 'utf8');
      for (const match of text.matchAll(REF_RE)) {
        if (retired.includes(match[1])) {
          offences.push(`${file}: §{${match[1]}}`);
        }
      }
      for (const match of text.matchAll(ANCHOR_RE)) {
        if (retired.includes(match[1])) {
          offences.push(`${file}: [#${match[1]}]`);
        }
      }
    }
    expect(offences).toEqual([]);
  });
});
```

Run it:

```bash
pnpm test src/plugins/__tests__/no-retired-slugs-in-source.test.ts
```

Expected: the only test is SKIPPED.

- [ ] **Step 1.7: Add rename-aware error to `remark-slug-xrefs.ts`**

Open `src/plugins/remark-slug-xrefs.ts` and add (at the top, after imports):

```ts
import { resolveRename } from '../../scripts/slug-renames.ts';
```

The existing plugin collapses `§{slug}` text nodes; it doesn't validate slugs. Slug resolution to URLs happens in `src/plugins/rehype-clause-xrefs.ts` against the harvested `clauseMap`. Locate that file:

```bash
cat src/plugins/rehype-clause-xrefs.ts | head -40
```

Inside the visitor that handles a `§{slug}` reference that does not resolve, add this block before whatever existing "unresolved" handler runs:

```ts
const rename = resolveRename(slug);
if (rename !== undefined) {
  // Retired slug. Emit a precise rename error rather than a generic
  // "unresolved" diagnostic.
  if (rename === null) {
    file.fail(
      `§{${slug}} references a slug that was retired with no replacement. ` +
      `See scripts/slug-renames.ts and Cook Standard v0.10 reorg.`,
      node,
      'remark-slug-xrefs:retired-slug',
    );
  } else {
    file.fail(
      `§{${slug}} renamed to §{${rename}} in Cook Standard v0.10. ` +
      `Update the reference in source. See scripts/slug-renames.ts.`,
      node,
      'remark-slug-xrefs:renamed-slug',
    );
  }
  return;
}
```

(Adjust to whatever the existing plugin's error-emission API is — the exact wording of `file.fail` calls may differ in the codebase. The contract is: emit a build error naming the new slug.)

- [ ] **Step 1.8: Wire build-emitted redirects**

Open `astro.config.mjs` and add at the top after imports:

```js
import { SLUG_RENAMES } from './scripts/slug-renames.ts';
import { SLUG_MAPPING } from './scripts/slug-mapping.ts';
```

Build a redirect map. For every retired slug, the old anchored URL is `/<old-chapter-path>/#<retired-slug>`. The new URL is `/<new-chapter-path>/#<replacement-slug>`. Astro Starlight accepts page-level redirects but not per-fragment redirects natively. Implement fragment-level redirects via a small client-side script that runs on the retired chapter pages.

For the minimum viable redirect: emit a `redirects` entry that maps the retired chapter slug to the new chapter's URL, and accept that anchor-level precision is lost during the migration. Document this limitation in the CS entry.

Add to `defineConfig`:

```js
  redirects: {
    '/03-syntactic-grammar/': '/04-toplevel/',
    '/04-recipes/':           '/06-recipes/',
    '/04a-chores/':           '/07-chores/',
    '/05-cross-recipe-references/': '/10-cross-recipe-references/',
    '/06-cook-lua-api/':      '/21-lua-api/',
    '/07-cross-cookfile-composition/': '/11-cross-cookfile-composition/',
    '/08-execution-model/':   '/13-two-phase/',
    '/09-standard-modules/':  '/27-catalogue/',
    '/01-notation/':          '/02-notation/',
    '/02-lexical/':           '/03-lexical/',
    '/appendix/b-rationale/': '/appendix/c-rationale/',
    '/appendix/c-examples/':  '/appendix/b-examples/',
    '/appendix/d-changes/':   '/appendix/e-changes/',
    '/appendix/e-pre-v1-checklist/': '/appendix/d-pre-v1-checklist/',
  },
```

(Anchor-level redirects can be improved post-merge by adding a client-side script that reads the URL fragment and rewrites to the new fragment per the `SLUG_RENAMES` map; that work is out of scope for this task. The page-level redirects ensure no chapter URL 404s.)

- [ ] **Step 1.9: Verify build still passes with infra in place**

```bash
pnpm build 2>&1 | tail -30
```

Expected: build SUCCEEDS. The redirects don't yet have destination pages (the new chapters don't exist), so Astro may warn about missing redirect targets. That's expected and resolves in Tasks 2–7.

- [ ] **Step 1.10: Commit**

```bash
git add scripts/slug-renames.ts scripts/slug-mapping.ts src/plugins/remark-slug-xrefs.ts src/plugins/__tests__/slug-renames-consistency.test.ts src/plugins/__tests__/no-retired-slugs-in-source.test.ts astro.config.mjs
git commit -m "$(cat <<'EOF'
docs(standard): pre-flight infra for v0.10 reorg (SHI-???)

Adds the slug-renames registry, build-emitted page redirects, and the
two plugin tests that gate slug consistency. The retired-slug-in-source
lint is wired but skipped until Task 8 deletes the old MDX files.

EOF
)"
```

(Substitute the actual SHI-NNN Linear ticket number if one is opened for the reorg.)

---

## Task 2: Front matter chapters (0, 1, 2)

**Files:**
- Modify: `src/content/docs/00-introduction.mdx`
- Create: `src/content/docs/01-conformance.mdx`
- Create: `src/content/docs/02-notation.mdx` (content of current `01-notation.mdx`)

- [ ] **Step 2.1: Trim `00-introduction.mdx`**

Open `src/content/docs/00-introduction.mdx`. Remove §0.7 Conformance (lines 49–58 in the current file — verify by reading the file fresh). The remainder of the file (purpose, scope, non-scope, normative/informative, versioning, architecture-relationship) stays.

Update the frontmatter `sidebar.order` if needed; current value `0` is fine.

- [ ] **Step 2.2: Create `01-conformance.mdx`**

```mdx
---
title: "§{conf} — Conformance"
sidebar:
  order: 1
---
# 1. Conformance [#conf]
> **Normative.** This chapter defines what it means for an implementation to conform to the Cook Standard.

## 1.1. Conformance criteria [#conf.criteria]

A conforming implementation:

1. MUST accept every Cookfile in the Standard's positive conformance corpus (`docs/standard/conformance/positive/`).
2. MUST reject every Cookfile in the Standard's negative conformance corpus (`docs/standard/conformance/negative/`) with a diagnostic that identifies the offending line. The exact wording is implementation-defined; the diagnostic class is normative.
3. For accepted Cookfiles, MUST produce a parse whose structural shape matches the expected shape recorded for each case.

See §{mods} of the Standard (Modules) for additional module-specific conformance requirements.

An implementation is said to **conform to Cook Standard v0.X** when, against the prose and corpus of the `cs-standard/v0.X` tag, it satisfies the three points above. The mechanism by which an implementation claims a version (a README statement, a library constant, a CLI flag, or any other surface) is implementation-defined and is not normatively required by this Standard.
```

(The section body is copied verbatim from the current §0.7. The two changes are: heading number `0.7` → `1.1`, slug `intro.conformance` → `conf.criteria`, and the cross-reference to `§{modules}` becomes `§{mods}` per the rename table.)

- [ ] **Step 2.3: Create `02-notation.mdx` by copy**

```bash
cp src/content/docs/01-notation.mdx src/content/docs/02-notation.mdx
```

Open `src/content/docs/02-notation.mdx` and change the frontmatter:

```yaml
---
title: "§{notation} — Notation and conventions"
sidebar:
  order: 2
---
```

(The slug `notation` stays. Only `sidebar.order` changes from `1` to `2`.)

The body is verbatim. Slug refs inside this file MUST be checked — search for any retired slug and rewrite per Reference A:

```bash
grep -n '§{grammar\.\|§{modules\.\|§{stdmods\.\|§{lexical.placeholders}' src/content/docs/02-notation.mdx
```

Expected: no hits in the notation chapter (it shouldn't reference content chapters by slug).

If hits appear, rewrite per the rename table.

- [ ] **Step 2.4: Build and verify**

```bash
pnpm build
```

Expected: build SUCCEEDS. Open `dist/02-notation/index.html` in a browser; verify the page renders.

- [ ] **Step 2.5: Commit**

```bash
git add src/content/docs/00-introduction.mdx src/content/docs/01-conformance.mdx src/content/docs/02-notation.mdx
git commit -m "$(cat <<'EOF'
docs(standard): create front matter chapters (0, 1, 2) for v0.10 reorg

Trims §0.7 out of introduction into its own Ch. 1 Conformance.
Copies notation to its new chapter slot. Old 01-notation.mdx
remains until Task 8 deletes the legacy files.

EOF
)"
```

---

## Task 3: Part I content (Chapters 3–12)

This is the largest task — ten chapter files, mostly content migration with slug rewriting. Sub-tasks are organised per chapter. Each sub-task ends with a `pnpm build` and a commit.

### Task 3.A: Chapter 3 — Lexical structure

**Files:**
- Create: `src/content/docs/03-lexical.mdx`

- [ ] **Step 3.A.1: Copy and trim**

```bash
cp src/content/docs/02-lexical.mdx src/content/docs/03-lexical.mdx
```

Open `src/content/docs/03-lexical.mdx`. Frontmatter:

```yaml
---
title: "§{lexical} — Lexical structure"
sidebar:
  order: 3
---
```

Delete the entirety of §2.11 (`## 2.11. Placeholders in shell text [#lexical.placeholders]` through the end of that section — the next section is §2.10 above it, so §2.11 is at the bottom of the file; delete down to the `## 2.X` heading that follows, or to EOF).

- [ ] **Step 3.A.2: Renumber the chapter to 3**

Use sed:

```bash
sed -i 's/^# 2\. Lexical structure/# 3. Lexical structure/' src/content/docs/03-lexical.mdx
sed -i 's/^## 2\./## 3./g' src/content/docs/03-lexical.mdx
sed -i 's/^### 2\./### 3./g' src/content/docs/03-lexical.mdx
sed -i 's/^#### 2\./#### 3./g' src/content/docs/03-lexical.mdx
```

Verify:

```bash
grep -E '^#+ [0-9]+' src/content/docs/03-lexical.mdx | head -15
```

Every heading number should now start with `3`.

- [ ] **Step 3.A.3: Rewrite slug refs to §11 / §12 surfaces**

Within this chapter, search for retired-slug references:

```bash
grep -nE '§\{(modules|grammar|stdmods)\.' src/content/docs/03-lexical.mdx
```

For each hit, rewrite per Reference A. Common ones in the lexical chapter:
- `§{grammar.recipe-syntax}` → `§{recipes.header-forms}`
- `§{grammar.config-blocks}` → `§{decl.config}`
- `§{grammar.use-declarations}` → `§{decl.use}`
- `§{grammar.import-declarations}` → `§{decl.import}`
- `§{grammar.register-blocks}` → `§{decl.register}`
- `§{grammar.step-dispatch}` → `§{steps.dispatch}`
- `§{grammar.top-level-ordering}` → `§{toplevel.ordering}`
- `§{grammar.top-level-module-call}` → `§{toplevel.module-call}`
- `§{grammar.config-composition}` → `§{decl.config-composition}`
- `§{lexical.placeholders}` → `§{phl.token}` (the placeholders section moved out)

- [ ] **Step 3.A.4: Build and verify**

```bash
pnpm build 2>&1 | tail -30
```

Expected: SUCCEEDS. Any remaining retired-slug warnings are about content still in the legacy files; those resolve in later tasks.

- [ ] **Step 3.A.5: Commit**

```bash
git add src/content/docs/03-lexical.mdx
git commit -m "docs(standard): create Ch. 3 Lexical structure (v0.10 reorg)"
```

### Task 3.B: Chapter 4 — Top-level structure

**Files:**
- Create: `src/content/docs/04-toplevel.mdx`

This chapter is new — it doesn't have a single source file. It aggregates §3.1, §3.2, §3.7.5, and §4.1.1 from the current spec.

- [ ] **Step 3.B.1: Scaffold the file**

```mdx
---
title: "§{toplevel} — Top-level structure"
sidebar:
  order: 4
---
# 4. Top-level structure [#toplevel]
> **Normative.** This chapter defines the top-level production of a Cookfile, the ordering rule for declarations and recipes, and the implicit-termination rule that bounds every body construct (recipe, chore, config, register).

## 4.1. Top-level production [#toplevel.overview]

<!-- TODO Step 3.B.2: paste current §3.1 here, renumbered -->

## 4.2. Top-level ordering [#toplevel.ordering]

<!-- TODO Step 3.B.3: paste current §3.2 here, renumbered -->

## 4.3. Body termination [#toplevel.termination]

<!-- TODO Step 3.B.4: paste current §4.1.1 here, renumbered -->

## 4.4. Top-level module calls [#toplevel.module-call]

<!-- TODO Step 3.B.5: paste current §3.7.5 here, renumbered -->
```

- [ ] **Step 3.B.2: Paste §4.1 (was §3.1 Grammar overview)**

Read lines 9–22 of `src/content/docs/03-syntactic-grammar.mdx`. Copy into Step 3.B.1's placeholder for §4.1. Renumber: change `## 3.1.` → `## 4.1.`, change `### Note 3.1.1` → `### Note 4.1.1`, etc. Replace the heading text "Grammar overview" with "Top-level production" (the prose stays).

Update the `[#grammar.overview]` marker to `[#toplevel.overview]` if it's still in the pasted text.

Rewrite slug refs in the pasted material per Reference A.

- [ ] **Step 3.B.3: Paste §4.2 (was §3.2 Top-level ordering)**

Read lines 24–54 of the current `03-syntactic-grammar.mdx` (the §3.2 section through the end of its examples). Copy into Step 3.B.1's placeholder for §4.2. Renumber `3.2` → `4.2`. Update `[#grammar.top-level-ordering]` → `[#toplevel.ordering]`. Rewrite slug refs.

- [ ] **Step 3.B.4: Paste §4.3 (was §4.1.1 Body termination)**

Read lines 34–72 of the current `04-recipes.mdx`. Copy the §4.1.1 section into Step 3.B.1's placeholder for §4.3. Renumber `4.1.1` → `4.3`. Update `[#recipes.termination]` → `[#toplevel.termination]`. Rewrite slug refs.

The body of §4.1.1 (currently scoped to recipes and configs) MUST be rephrased to scope to all body constructs uniformly: recipe body, chore body, config body, register-block body. Replace any text that says "A `recipe_body` and a `config_body` extend..." with "A body construct (`recipe_body`, `chore_body`, `config_body`, `register_body`) extends...".

Add `register` to the column-0 keyword list if not already present.

Add `pre-v1.*` notes about the rephrasing if material; otherwise leave existing notes as they were.

- [ ] **Step 3.B.5: Paste §4.4 (was §3.7.5 Top-level module calls)**

Read lines 221–250 of the current `03-syntactic-grammar.mdx` (the §3.7.5 section). Copy into Step 3.B.1's placeholder for §4.4. Renumber `3.7.5` → `4.4`. Update `[#grammar.top-level-module-call]` → `[#toplevel.module-call]`. Rewrite slug refs (in particular, `§{grammar.register-blocks.splicing}` → `§{decl.register-splicing}`).

- [ ] **Step 3.B.6: Build and verify**

```bash
pnpm build 2>&1 | tail -30
```

Expected: SUCCEEDS, with possible warnings about the new chapter forward-referencing chapters that don't yet exist. That's fine.

Open `dist/04-toplevel/index.html` and verify all four sections render with correct numbering.

- [ ] **Step 3.B.7: Commit**

```bash
git add src/content/docs/04-toplevel.mdx
git commit -m "docs(standard): create Ch. 4 Top-level structure (v0.10 reorg)"
```

### Task 3.C: Chapter 5 — Declarations

**Files:**
- Create: `src/content/docs/05-declarations.mdx`

This chapter aggregates §3.4–§3.7 from `03-syntactic-grammar.mdx`.

- [ ] **Step 3.C.1: Scaffold the file**

```mdx
---
title: "§{decl} — Declarations"
sidebar:
  order: 5
---
# 5. Declarations [#decl]
> **Normative.** This chapter defines the four top-level declaration forms — `use`, `import`, `config`, and `register` — along with their bodies and composition rules. Each declaration's effect on Cook Lua state, on cross-Cookfile composition, or on the register-phase program is specified in the chapter named in the cross-reference.

## 5.1. `use` declarations [#decl.use]

<!-- §3.4 -->

## 5.2. `import` declarations [#decl.import]

<!-- §3.5 -->

## 5.3. `config` blocks [#decl.config]

<!-- §3.6 -->

### 5.3.1. Config-block composition [#decl.config-composition]

<!-- §3.6.1 -->

## 5.4. `register` blocks [#decl.register]

<!-- §3.7 -->

### 5.4.1. Splicing semantics [#decl.register-splicing]

<!-- §3.7.X (current "Splicing semantics" subsection of §3.7) -->
```

- [ ] **Step 3.C.2: Populate §5.1 from current §3.4**

Read the §3.4 section in `03-syntactic-grammar.mdx`. Copy verbatim into Step 3.C.1's placeholder. Renumber `3.4` → `5.1`. Update `[#grammar.use-declarations]` → `[#decl.use]`. Rewrite slug refs.

- [ ] **Step 3.C.3: Populate §5.2 from current §3.5**

Same process for §3.5. Renumber → `5.2`. Update slug marker → `[#decl.import]`.

- [ ] **Step 3.C.4: Populate §5.3 + §5.3.1 from current §3.6 + §3.6.1**

Same process. Renumber → `5.3` and `5.3.1`. Update slug markers → `[#decl.config]` and `[#decl.config-composition]`.

- [ ] **Step 3.C.5: Populate §5.4 + §5.4.1 from current §3.7**

Same process. Renumber → `5.4` and `5.4.1`. Update slug markers → `[#decl.register]` and `[#decl.register-splicing]`.

**Important:** the current §3.7 also contains §3.7.5 (top-level module_call), which already moved to Ch. 4 in Task 3.B. Do NOT copy §3.7.5 here; it's a sibling of §3.7, not a sub-section of register-blocks under the new map.

- [ ] **Step 3.C.6: Build and verify**

```bash
pnpm build
```

Expected: SUCCEEDS. Verify `dist/05-declarations/index.html` renders all four declaration forms.

- [ ] **Step 3.C.7: Commit**

```bash
git add src/content/docs/05-declarations.mdx
git commit -m "docs(standard): create Ch. 5 Declarations (v0.10 reorg)"
```

### Task 3.D: Chapter 6 — Recipes

**Files:**
- Create: `src/content/docs/06-recipes.mdx`

This chapter takes the recipe-header and dep-list material from current `04-recipes.mdx` §4.1 and §4.2, plus the §3.8 grammar walk-through merged in. It also promotes Note 4.4.2 (region rule) to a numbered subsection.

- [ ] **Step 3.D.1: Scaffold the file**

```mdx
---
title: "§{recipes} — Recipes"
sidebar:
  order: 6
---
# 6. Recipes [#recipes]
> **Normative.** This chapter defines the shape of a recipe declaration: the header, the optional dependency list, the body's two-region structure, and the at-most-one-ingredients rule. Step kinds are defined in §{steps}; chores are in §{chores}.

## 6.1. Recipe header [#recipes.header-forms]

<!-- merge §3.8 grammar overview prose with current §4.1 -->

## 6.2. Dependency list [#recipes.dep-list]

<!-- §4.2 -->

## 6.3. Recipe-body region rule [#recipes.region-rule]

<!-- promoted from current Note 4.4.2 + App. A.3 region-ordering rule -->
```

- [ ] **Step 3.D.2: Populate §6.1 from §3.8 + §4.1**

Read §3.8 from current `03-syntactic-grammar.mdx`. Read §4.1 from current `04-recipes.mdx`. Merge: §3.8's grammar walk-through goes first; §4.1's elaboration follows. Avoid duplicating the duplicate-name rule and reserved-segment rule (each appears in both today; one normative copy here suffices).

Renumber to §6.1. Update slug marker → `[#recipes.header-forms]`. Rewrite slug refs.

- [ ] **Step 3.D.3: Populate §6.2 from current §4.2**

Same process. Renumber → §6.2. Slug `recipes.dep-list` is preserved.

- [ ] **Step 3.D.4: Populate §6.3 — promote Note 4.4.2**

The current Note 4.4.2 reads:

> A `recipe_body` MUST be ordered into two consecutive regions, each of which MAY be empty:
> 1. **Declarative region** — register-phase steps: ...
> 2. **Imperative region** — execute-phase steps: ...
> Once any imperative-region step appears in the body, no declarative-region step is permitted afterward in the same recipe. ...

Promote this to a full numbered subsection §6.3 titled "Recipe-body region rule" with slug `[#recipes.region-rule]`. The "Note" wrapper is dropped; the prose stands as normative.

Also fold in the App. A.3 "Region ordering rule" text (it's currently a normative-bold prose block beneath the grammar block). The chapter has the prose; App. A keeps the grammar.

Rewrite slug refs.

- [ ] **Step 3.D.5: Build and verify**

```bash
pnpm build
```

Expected: SUCCEEDS. Verify the recipe chapter renders.

- [ ] **Step 3.D.6: Commit**

```bash
git add src/content/docs/06-recipes.mdx
git commit -m "docs(standard): create Ch. 6 Recipes (v0.10 reorg)"
```

### Task 3.E: Chapter 7 — Chores

**Files:**
- Rename: `src/content/docs/04a-chores.mdx` → `src/content/docs/07-chores.mdx` (preserve blame via git mv)

- [ ] **Step 3.E.1: Move the file**

```bash
git mv src/content/docs/04a-chores.mdx src/content/docs/07-chores.mdx
```

Open the new file. Frontmatter:

```yaml
---
title: "§{chores} — Chores"
sidebar:
  order: 7
---
```

(Change `order: 4.5` → `order: 7`.)

- [ ] **Step 3.E.2: Renumber inside the file**

```bash
sed -i 's/^# 4a\. Chores/# 7. Chores/' src/content/docs/07-chores.mdx
sed -i 's/^## 4a\./## 7./g' src/content/docs/07-chores.mdx
sed -i 's/^### 4a\./### 7./g' src/content/docs/07-chores.mdx
sed -i 's/^#### 4a\./#### 7./g' src/content/docs/07-chores.mdx
```

Verify:

```bash
grep -E '^#+ [0-9]+' src/content/docs/07-chores.mdx | head -10
```

- [ ] **Step 3.E.3: Update the banned-steps cross-references**

Current §4a.2 cross-references `§ 4.X` for each step kind it bans (ingredients, cook, plate, test). Rewrite those to point at §{steps.*} sub-slugs:

- `§ 4.3` → `§{steps.ingredients}`
- `§ 4.5–4.6` → `§{steps.cook-single}`
- `§ 4.7` → `§{steps.plate}`
- `§ 4.8` → `§{steps.test}`

Also update the "allowed step kinds" table's `§` column to use the new slug-based refs.

Rewrite any other retired slug refs per Reference A.

- [ ] **Step 3.E.4: Build and verify**

```bash
pnpm build
```

- [ ] **Step 3.E.5: Commit**

```bash
git add src/content/docs/07-chores.mdx
git commit -m "docs(standard): promote chores to Ch. 7 (v0.10 reorg)"
```

### Task 3.F: Chapter 8 — Step kinds

**Files:**
- Create: `src/content/docs/08-step-kinds.mdx`

This chapter aggregates current §3.9 (step dispatch) + §4.3 (ingredients) + §4.4 (step-kinds overview table) + §4.5–§4.10 (each step kind). Body bundling (current §4.9.3) is OMITTED — it moves to Ch. 15.

- [ ] **Step 3.F.1: Scaffold the file**

```mdx
---
title: "§{steps} — Step kinds"
sidebar:
  order: 8
---
# 8. Step kinds [#steps]
> **Normative.** This chapter defines every step kind that may appear in a recipe body or a chore body, the dispatch cascade that classifies each `Content` token, and the iteration-mode rules for `cook`, `plate`, and `test` steps. The execute-phase behaviour of a body's compiled units — including body bundling — is specified in §{exec.groups}.

## 8.1. Step-dispatch cascade [#steps.dispatch]

<!-- §3.9 -->

## 8.2. Ingredients [#steps.ingredients]

<!-- §4.3 -->

## 8.3. Step kinds — overview [#steps.overview]

<!-- §4.4 overview table only, region rule promoted out -->

## 8.4. `cook` — single-output form [#steps.cook-single]

<!-- §4.5 -->

### 8.4.1. Iteration mode [#steps.iteration-mode]

<!-- §4.5.1 -->

## 8.5. `cook` — multi-output form [#steps.cook-multi]

<!-- §4.6 -->

## 8.6. `plate` step [#steps.plate]

<!-- §4.7 (minus the iteration-mode subsection — see §8.6.1) -->

### 8.6.1. Plate/test iteration mode [#steps.iteration-mode-plate-test]

<!-- §4.7.1 -->

## 8.7. `test` step [#steps.test]

<!-- §4.8 -->

## 8.8. Lua steps — line and block forms [#steps.lua]

<!-- §4.9 EXCLUDING §4.9.3 body bundling (moves to §15) -->

### 8.8.1. Delimitation [#steps.lua-delimitation]

<!-- §4.9.1 -->

### 8.8.2. Execution phase [#steps.lua-execution-phase]

<!-- §4.9.2 (with cross-ref to §15 for bundling) -->

### 8.8.3. Examples [#steps.lua-examples]

<!-- §4.9.4 -->

## 8.9. Shell steps — plain and interactive [#steps.shell]

<!-- §4.10 -->
```

- [ ] **Step 3.F.2: Populate §8.1 from §3.9**

Read §3.9 of current `03-syntactic-grammar.mdx`. Copy into §8.1. Renumber. Update slug marker → `[#steps.dispatch]`. Rewrite slug refs.

- [ ] **Step 3.F.3: Populate §8.2 from §4.3**

Read §4.3 (Ingredients) of current `04-recipes.mdx`. Copy into §8.2. Renumber.

- [ ] **Step 3.F.4: Populate §8.3 from §4.4 overview only**

Read §4.4 of current `04-recipes.mdx`. Copy ONLY the introductory prose and the table of step kinds (do NOT copy Note 4.4.2 — it's promoted to §6.3 by Task 3.D). Renumber → §8.3.

- [ ] **Step 3.F.5: Populate §8.4 + §8.4.1 from §4.5 + §4.5.1**

Same process. Renumber. Slug markers → `[#steps.cook-single]` and `[#steps.iteration-mode]`.

- [ ] **Step 3.F.6: Populate §8.5 from §4.6**

Same. Slug marker → `[#steps.cook-multi]`.

- [ ] **Step 3.F.7: Populate §8.6 + §8.6.1 from §4.7 + §4.7.1**

Same. Slug markers → `[#steps.plate]` and `[#steps.iteration-mode-plate-test]`.

- [ ] **Step 3.F.8: Populate §8.7 from §4.8**

Same. Slug marker → `[#steps.test]`.

- [ ] **Step 3.F.9: Populate §8.8 + sub-sections from §4.9 EXCLUDING §4.9.3**

Copy §4.9, §4.9.1, §4.9.2, and §4.9.4 (NOT §4.9.3). Renumber. Slug markers as scaffolded.

In §8.8.2 (was §4.9.2), the text references `§{recipes.body-bundling}` for the body-unit composition rule. Rewrite this to `§{exec.body-bundling}` (or whichever slug Task 4 assigns to the body-bundling subsection of Ch. 15). Use `§{exec.body-bundling}` as the target slug; record it in `scripts/slug-mapping.ts` if not already there:

```ts
  'sec-15-new-body-bundling': 'exec.body-bundling',
```

- [ ] **Step 3.F.10: Populate §8.9 from §4.10**

Same. Slug marker → `[#steps.shell]`.

- [ ] **Step 3.F.11: Drop the "module-call removed" subsection**

Current §4.11 documents the removed `module_call` step kind for migration purposes. This section moves NOT to step-kinds chapter but to Annex D (Pre-1.0 checklist) — or rather, it stays in App. C (Rationale) as a historical note. Decision: keep §4.11 content in App. C Rationale as a §C.X.4.11 entry, retire the §4.11 chapter location. The rationale entry was already present as `rationale.module-call-heuristic`; expand it in Task 7 if needed.

Do NOT include §4.11 in §8.

- [ ] **Step 3.F.12: Build and verify**

```bash
pnpm build
```

- [ ] **Step 3.F.13: Commit**

```bash
git add src/content/docs/08-step-kinds.mdx scripts/slug-mapping.ts
git commit -m "docs(standard): create Ch. 8 Step kinds (v0.10 reorg)"
```

### Task 3.G: Chapter 9 — Placeholders

**Files:**
- Create: `src/content/docs/09-placeholders.mdx`

Consolidates current §2.11 (token), §6.7 (cook-step shell-text placeholders), and §6.7.1 (plate/test shell-block placeholders) into one chapter.

- [ ] **Step 3.G.1: Scaffold the file**

```mdx
---
title: "§{phl} — Placeholders"
sidebar:
  order: 9
---
# 9. Placeholders [#phl]
> **Normative.** This chapter defines the placeholder surface — the `$<IDENT>` token, the resolution rules that route it to a builtin / recipe / env var, and the per-step-kind tables that say which placeholder shapes are valid in which iteration mode. Cross-recipe name reference shapes that share the same `$<...>` syntax are specified in §{xref}.

## 9.1. Placeholder token [#phl.token]

<!-- §2.11 -->

## 9.2. Resolution [#phl.resolution]

<!-- pointer to §{xref.resolution}; this section restates the closed-function rule (every well-lexed placeholder resolves to a known thing or fails loudly) and cross-references §10 for the full cascade -->

## 9.3. Cook-step placeholders [#phl.cook-step]

<!-- §6.7 -->

## 9.4. Plate and test placeholders [#phl.plate-test]

<!-- §6.7.1 -->
```

- [ ] **Step 3.G.2: Populate §9.1 from §2.11**

Read §2.11 of current `02-lexical.mdx`. Copy into §9.1. Update slug marker → `[#phl.token]`. Rewrite refs.

- [ ] **Step 3.G.3: Populate §9.2 — resolution pointer**

§9.2 is short: it restates the closed-function rule and points readers to §{xref.resolution} for the full cascade. Write:

```mdx
## 9.2. Resolution [#phl.resolution]

Placeholder resolution is the closed function defined in §{xref.resolution}. Every well-lexed `$<IDENT>` either resolves to a known thing (a builtin shape from §{phl.cook-step} or §{phl.plate-test}, a recipe name from §{xref.name-references}, or a declared env var from §{xref.env-namespace}) or fails loudly. There is no silent fallthrough.
```

- [ ] **Step 3.G.4: Populate §9.3 from §6.7**

Read §6.7 of current `06-cook-lua-api.mdx`. Copy into §9.3. Renumber. Update slug marker → `[#phl.cook-step]`. Note: the original §6.7 has a "Phase" header note saying substitution is performed by the code generator at register time. Keep this note as informative.

Rewrite any `§{lua.shell-placeholders}` cross-references that appear in OTHER chapters; the slug `lua.shell-placeholders` is removed and references should now point at `§{phl.cook-step}` or `§{phl.plate-test}`. Add the old slug to `scripts/slug-renames.ts`:

```ts
  'lua.shell-placeholders':          'phl.cook-step',
  'lua.shell-placeholders-plate-test': 'phl.plate-test',
```

- [ ] **Step 3.G.5: Populate §9.4 from §6.7.1**

Same process. Slug marker → `[#phl.plate-test]`.

- [ ] **Step 3.G.6: Build and verify**

```bash
pnpm build
```

- [ ] **Step 3.G.7: Commit**

```bash
git add src/content/docs/09-placeholders.mdx scripts/slug-renames.ts
git commit -m "docs(standard): create Ch. 9 Placeholders (v0.10 reorg)"
```

### Task 3.H: Chapter 10 — Cross-recipe references

**Files:**
- Rename: `src/content/docs/05-cross-recipe-references.mdx` → `src/content/docs/10-cross-recipe-references.mdx` via copy

- [ ] **Step 3.H.1: Copy and renumber**

```bash
cp src/content/docs/05-cross-recipe-references.mdx src/content/docs/10-cross-recipe-references.mdx
```

Frontmatter:

```yaml
---
title: "§{xref} — Cross-recipe references"
sidebar:
  order: 10
---
```

Renumber inside the file:

```bash
sed -i 's/^# 5\. Cross-recipe references/# 10. Cross-recipe references/' src/content/docs/10-cross-recipe-references.mdx
sed -i 's/^## 5\./## 10./g' src/content/docs/10-cross-recipe-references.mdx
sed -i 's/^### 5\./### 10./g' src/content/docs/10-cross-recipe-references.mdx
```

- [ ] **Step 3.H.2: Rewrite slug refs**

Within this chapter, expect references to:
- `§{lua.shell-placeholders}` → `§{phl.cook-step}`
- `§{modules}` → `§{comp}` (cross-Cookfile composition)
- `§{modules.qualified-refs}` → `§{comp.qualified-refs}`
- `§{lua.path-helpers}` (and `lua.path-X`) → preserved
- `§{xref.X}` slugs are preserved

Search and rewrite:

```bash
grep -nE '§\{(modules|grammar|stdmods|lua\.shell-placeholders)' src/content/docs/10-cross-recipe-references.mdx
```

- [ ] **Step 3.H.3: Build, verify, commit**

```bash
pnpm build
git add src/content/docs/10-cross-recipe-references.mdx
git commit -m "docs(standard): renumber Ch. 10 Cross-recipe references (v0.10 reorg)"
```

### Task 3.I: Chapter 11 — Cross-Cookfile composition

**Files:**
- Create: `src/content/docs/11-cross-cookfile-composition.mdx`

Strips §7.6 (workspace root) and §7.7 (cache portability) from current `07-cross-cookfile-composition.mdx`.

- [ ] **Step 3.I.1: Copy and trim**

```bash
cp src/content/docs/07-cross-cookfile-composition.mdx src/content/docs/11-cross-cookfile-composition.mdx
```

Open the new file. Delete §7.6 (Workspace root determination) and §7.7 (Cache portability invariants) — these move to Part II.

Frontmatter:

```yaml
---
title: "§{comp} — Cross-Cookfile composition"
sidebar:
  order: 11
---
```

- [ ] **Step 3.I.2: Renumber chapter to 11**

```bash
sed -i 's/^# 7\. Cross-Cookfile composition/# 11. Cross-Cookfile composition/' src/content/docs/11-cross-cookfile-composition.mdx
sed -i 's/^## 7\./## 11./g' src/content/docs/11-cross-cookfile-composition.mdx
sed -i 's/^### 7\./### 11./g' src/content/docs/11-cross-cookfile-composition.mdx
```

- [ ] **Step 3.I.3: Rewrite slug markers**

Update every `[#modules.X]` heading marker per Reference A:
- `[#modules]` → `[#comp]`
- `[#modules.overview]` → `[#comp.overview]`
- `[#modules.import-declaration]` → `[#comp.import]`
- `[#modules.qualified-refs]` → `[#comp.qualified-refs]`
- `[#modules.use-scope]` → `[#mods.use-scope]` (NB: this stays as a stub-pointer-to-Ch.-12; the slug moves to Ch. 12)
- `[#modules.duplicates-and-cycles]` → `[#comp.duplicates-and-cycles]`

For §11.4 (was §7.4 Use scope) — the content is moving to §12 in Task 3.J. In this chapter, leave a short stub:

```mdx
## 11.4. `use` scope is lexical per Cookfile

`use` scope is defined in §{mods.use-scope}.
```

- [ ] **Step 3.I.4: Rewrite cross-references**

```bash
grep -nE '§\{(modules|lua\.cook-load-module|lua\.use-env|stdmods)' src/content/docs/11-cross-cookfile-composition.mdx
```

Rewrite per Reference A. `§{lua.use-env}` → `§{mods.use}`.

- [ ] **Step 3.I.5: Build, verify, commit**

```bash
pnpm build
git add src/content/docs/11-cross-cookfile-composition.mdx
git commit -m "docs(standard): create Ch. 11 Cross-Cookfile composition (v0.10 reorg)"
```

### Task 3.J: Chapter 12 — Modules

**Files:**
- Create: `src/content/docs/12-modules.mdx`

This chapter aggregates current §6.8 (use declaration), parts of current §7.4 (use scope), the new lifecycle subsection, and a pointer to §27 catalogue.

- [ ] **Step 3.J.1: Scaffold the file**

```mdx
---
title: "§{mods} — Modules"
sidebar:
  order: 12
---
# 12. Modules [#mods]
> **Normative.** This chapter defines the `use` declaration, module resolution, the load-time lifecycle, and the catalogue index that points at Part IV. The Lua API call `cook.load_module` that implements resolution at runtime is defined in §{lua.both}; this chapter specifies what the surface declaration means and when module code runs.

## 12.1. The `use` declaration [#mods.use]

<!-- §6.8 -->

## 12.2. `use` scope is lexical per Cookfile [#mods.use-scope]

<!-- §7.4 -->

## 12.3. Module lifecycle [#mods.lifecycle]

<!-- NEW NORMATIVE PROSE — see Step 3.J.4 -->

## 12.4. Built-in modules [#mods.builtin]

<!-- §6.8.1 -->

## 12.5. Local modules [#mods.local]

<!-- §6.8.2 -->

## 12.6. Standard module catalogue [#mods.catalogue-index]

<!-- short pointer to §27 -->
```

- [ ] **Step 3.J.2: Populate §12.1 from §6.8**

Read §6.8 of current `06-cook-lua-api.mdx`. Copy into §12.1. Renumber. Update slug marker → `[#mods.use]`. The Normative banner in current §6.8 ("This section defines the `use` declaration ...") can be deleted since the chapter-level banner covers it.

Note: the current §6.8 prose talks about `module_call` step kind. CS-0072 removed `module_call` from recipe-body dispatch. The §12.1 prose MUST be updated to remove the recipe-body module-call references and instead point at §{toplevel.module-call} (top-level form) and §{decl.register} (register-block form) as the surfaces where module functions are called.

Rewrite slug refs.

- [ ] **Step 3.J.3: Populate §12.2 from §7.4**

Read §7.4 of current `07-cross-cookfile-composition.mdx`. Copy into §12.2. Update slug marker → `[#mods.use-scope]`. The CS-0066/0069 amendments inside §7.4 are part of this section.

- [ ] **Step 3.J.4: Write §12.3 Module lifecycle (NEW NORMATIVE PROSE)**

This subsection is new normative material. ~40 lines. Source the prose from:
- Current §6.3.4 `cook.load_module` body (caching, cycle detection, exec-vs-register exposure)
- Current §6.8 last paragraph (`init` hook contract)
- Current §7.4 (per-VM load semantics)

Write it as:

```mdx
## 12.3. Module lifecycle [#mods.lifecycle]

This section specifies when module code executes and what scope it observes.

### 12.3.1. Load order [#mods.lifecycle.load-order]

A conforming implementation MUST execute module code in the following order, for each Cookfile in a workspace, before any recipe body of that Cookfile registers:

1. For each `use <name>` declaration (§{mods.use}) in source order, resolve `<name>` against the search paths of §{mods.local} and §{mods.builtin}. The first hit wins.
2. Execute the resolved module's top-level chunk on the calling Cookfile's register-phase Lua VM. The chunk MUST return a Lua table; a chunk that does not return a table is a load-time error. The returned table is bound under the alias derived from `<name>` per §{mods.use}.
3. If the returned table has an `init` field bound to a function value, call `init()` exactly once on the register-phase VM. The `init()` call MAY read `cook.cache` (§{lua.both}) and MAY call register-phase Cook Lua API (§{lua.reg}).
4. Repeat for the next `use <name>` declaration in source order. A later `use` declaration's module body MAY observe state established by an earlier `use` declaration's `init()`.

Module code MUST NOT register work units during top-level chunk execution. Work-unit registration is the responsibility of recipe bodies; modules typically expose functions (e.g. target makers) that recipe bodies call.

### 12.3.2. Per-VM caching [#mods.lifecycle.caching]

A conforming implementation MUST cache the resolved module table by `(working_dir, name)`. A second `use <name>` within the same Cookfile returns the same table without re-reading or re-evaluating the file; the top-level chunk and `init()` MUST run **at most once** per `(working_dir, name)` per Lua VM as a direct consequence of this caching contract. The cache is per-VM: a register-phase VM and an execute-phase VM that both load the same module each independently load the module top-level chunk once.

### 12.3.3. Cycle detection [#mods.lifecycle.cycles]

A conforming implementation MUST detect `cook.load_module` cycles. A cycle is a re-entrant call to `cook.load_module(name)` that occurs while an earlier load of the same `(working_dir, name)` is still in flight on this VM — that is, the earlier call's module body or `init()` has not yet returned. On detection, the implementation MUST raise a runtime error whose message begins with `module cycle detected:` and includes the cycle path rendered as the in-flight module names joined by ` -> `, with the offending re-entered name appended.

Detection MUST survive recoverable errors: when a module's body or `init()` raises, the implementation MUST remove that module from the in-flight set so a later retry on the same VM can proceed (and a genuine subsequent cycle remains detectable).

### 12.3.4. Cross-VM rehydration [#mods.lifecycle.rehydration]

When a module's `init()` records state into `cook.cache` (§{lua.both}) during register-phase load, an execute-phase VM that later loads the same module MUST observe that cached state. The execute-phase implementation MAY use in-memory-only storage scoped to the worker's lifetime (no cross-invocation persistence). [Pinned by CS-0070.]

When a module records transitive-link info via `cook.export` (§{lua.both}) at register time, an execute-phase recipe body that calls `cook.import` MUST observe the recorded info. [Pinned by CS-0071.]
```

Use this prose verbatim, adapting paragraph numbering if a heading was reused elsewhere.

- [ ] **Step 3.J.5: Populate §12.4 + §12.5**

Copy §6.8.1 (Built-in modules) into §12.4. Copy §6.8.2 (Local modules) into §12.5. Renumber. Update slug markers.

- [ ] **Step 3.J.6: Write §12.6 catalogue index pointer**

```mdx
## 12.6. Standard module catalogue [#mods.catalogue-index]

A conforming implementation MAY ship a curated set of **blessed modules** distributed through the official LuaRocks-backed registry (`rocks.usecook.com`). The Standard governs each blessed module's public surface in Part IV (Standard module catalogue, §{cat}). The current catalogue is:

| Module | Section |
|---|---|
| `cc` | §{cat.cc} |

Future blessed modules are added additively to the catalogue without disturbing the resolution rules in this chapter.
```

- [ ] **Step 3.J.7: Build, verify, commit**

```bash
pnpm build
git add src/content/docs/12-modules.mdx
git commit -m "$(cat <<'EOF'
docs(standard): create Ch. 12 Modules (v0.10 reorg)

Includes new normative prose for §12.3 Module lifecycle (~40 lines)
synthesised from CS-0066/0069/0070/0071 amendments and the current
scattered prose in §6.3.4, §6.8, and §7.4.

EOF
)"
```

---

## Task 4: Part II content (Chapters 13–20)

These chapters mostly extract from current `08-execution-model.mdx` with two cross-Part imports: §4.9.3 body bundling (from current `04-recipes.mdx`) into Ch. 15, and §7.7 cache portability (from current `07-cross-cookfile-composition.mdx`) into Ch. 17.

### Task 4.A: Chapter 13 — Two-phase model

**Files:**
- Create: `src/content/docs/13-two-phase.mdx`

- [ ] **Step 4.A.1: Scaffold**

```mdx
---
title: "§{exec.phases} — Two-phase model"
sidebar:
  order: 13
---
# 13. Two-phase model [#exec.phases]
> **Normative.** This chapter defines the register/execute split that governs every Cookfile evaluation. The phase classification of every Lua-bearing surface form is consolidated in the table at §{exec.phases.classification}.

## 13.1. Two-phase execution [#exec.two-phase]

<!-- §8.1 -->

## 13.2. Phase classification table [#exec.phases.classification]

<!-- §8.1.2 -->
```

- [ ] **Step 4.A.2: Populate §13.1 from §8.1**

Copy. Renumber to §13.1. Slug marker `[#exec.two-phase]` is preserved.

- [ ] **Step 4.A.3: Populate §13.2 from §8.1.2**

Copy. Renumber to §13.2. Slug marker → `[#exec.phases.classification]`. Add the new slug to `scripts/slug-mapping.ts`:

```ts
  'sec-13-classification': 'exec.phases.classification',
```

(The current `exec.phase-classification` slug is preserved if anything refs it; otherwise rename. Search source.)

- [ ] **Step 4.A.4: Build, commit**

```bash
pnpm build
git add src/content/docs/13-two-phase.mdx scripts/slug-mapping.ts
git commit -m "docs(standard): create Ch. 13 Two-phase model (v0.10 reorg)"
```

### Task 4.B: Chapter 14 — Capture mode

**Files:**
- Create: `src/content/docs/14-capture-mode.mdx`

- [ ] **Step 4.B.1: Copy from §8.2**

```mdx
---
title: "§{exec.capture} — Capture mode"
sidebar:
  order: 14
---
# 14. Capture mode [#exec.capture]
> **Normative.** This chapter defines the determinism contract for register-phase Lua execution.

## 14.1. Capture-mode semantics [#exec.capture-mode]

<!-- §8.2 -->
```

Populate §14.1 from current §8.2.

- [ ] **Step 4.B.2: Build, commit**

```bash
pnpm build
git add src/content/docs/14-capture-mode.mdx
git commit -m "docs(standard): create Ch. 14 Capture mode (v0.10 reorg)"
```

### Task 4.C: Chapter 15 — Step groups and parallelism

**Files:**
- Create: `src/content/docs/15-step-groups.mdx`

Merges §8.3 (step groups) and §4.9.3 (body bundling).

- [ ] **Step 4.C.1: Scaffold**

```mdx
---
title: "§{exec.groups} — Step groups and parallelism"
sidebar:
  order: 15
---
# 15. Step groups and parallelism [#exec.groups]
> **Normative.** This chapter defines how registered work units are grouped for parallel execution, how the recipe body's imperative region compiles to body units, and how the two interact.

## 15.1. Step groups [#exec.step-groups]

<!-- §8.3 -->

## 15.2. Body bundling [#exec.body-bundling]

<!-- §4.9.3 -->
```

- [ ] **Step 4.C.2: Populate §15.1 from §8.3**

Slug marker `[#exec.step-groups]` preserved. Rewrite refs.

- [ ] **Step 4.C.3: Populate §15.2 from §4.9.3**

Copy current §4.9.3 (Body bundling) from `04-recipes.mdx`. Renumber to §15.2. Slug marker `[#exec.body-bundling]` (new). The slug `recipes.body-bundling` retires; add to `scripts/slug-renames.ts`:

```ts
  'recipes.body-bundling':           'exec.body-bundling',
```

- [ ] **Step 4.C.4: Build, commit**

```bash
pnpm build
git add src/content/docs/15-step-groups.mdx scripts/slug-renames.ts
git commit -m "docs(standard): create Ch. 15 Step groups + body bundling (v0.10 reorg)"
```

### Task 4.D: Chapter 16 — Cross-recipe ordering and interactive drain

**Files:**
- Create: `src/content/docs/16-ordering-drain.mdx`

Aggregates §8.4 + §8.4.1 + §8.5.

- [ ] **Step 4.D.1: Scaffold and populate**

```mdx
---
title: "§{exec.ord} — Cross-recipe ordering and interactive drain"
sidebar:
  order: 16
---
# 16. Cross-recipe ordering and interactive drain [#exec.ord]
> **Normative.** This chapter defines how cross-recipe dependencies enforce execution ordering, the output-path-uniqueness rule that prevents silent races, and the interactive-step drain behaviour.

## 16.1. Cross-recipe ordering [#exec.cross-recipe-ordering]

<!-- §8.4 -->

### 16.1.1. Output-path uniqueness across recipes [#exec.output-uniqueness]

<!-- §8.4.1 -->

## 16.2. Interactive step draining [#exec.interactive-drain]

<!-- §8.5 -->
```

Populate each section by copying from current §8.4, §8.4.1, §8.5. Renumber. Slug markers `[#exec.cross-recipe-ordering]`, `[#exec.output-uniqueness]`, `[#exec.interactive-drain]` are preserved.

- [ ] **Step 4.D.2: Build, commit**

```bash
pnpm build
git add src/content/docs/16-ordering-drain.mdx
git commit -m "docs(standard): create Ch. 16 Cross-recipe ordering + drain (v0.10 reorg)"
```

### Task 4.E: Chapter 17 — Cache semantics

**Files:**
- Create: `src/content/docs/17-cache.mdx`

Aggregates §8.6 + §8.6.1 + §8.6.3 + §8.6.4 (note: §8.6.2 is informative Note 8.6.2 — keep it) PLUS §7.7 (cache portability invariants).

- [ ] **Step 4.E.1: Scaffold**

```mdx
---
title: "§{exec.cache} — Cache semantics"
sidebar:
  order: 17
---
# 17. Cache semantics [#exec.cache]
> **Normative.** This chapter defines the abstract cache contract, the integrity rule for restoring artifacts from a content-addressable store, the discovered-inputs mechanism, the test-unit caching contract, and the portability invariants that allow a cache to travel between machines and workspace relocations.

## 17.1. Cache semantics (abstract) [#exec.cache.abstract]

<!-- §8.6 prose (down to but not including §8.6.1) -->

### 17.1.1. Tool-binary fingerprinting (informative) [#exec.cache.tool-binary]

<!-- §8.6 Note 8.6.2 -->

## 17.2. Cache integrity [#exec.cache.integrity]

<!-- §8.6.1 -->

## 17.3. Discovered inputs [#exec.cache.discovered-inputs]

<!-- §8.6.3 -->

## 17.4. Test-unit caching [#exec.cache.test-unit]

<!-- §8.6.4 -->

## 17.5. Cache portability invariants [#exec.cache.portability]

<!-- §7.7 -->
```

- [ ] **Step 4.E.2: Populate §17.1 + Note from §8.6**

Slug marker `[#exec.cache.abstract]` is new. Update `scripts/slug-mapping.ts`:

```ts
  'sec-17-new':            'exec.cache',
  'sec-17-new-1':          'exec.cache.abstract',
  'sec-17-new-1-1':        'exec.cache.tool-binary',
  'sec-17-new-2':          'exec.cache.integrity',
  'sec-17-new-3':          'exec.cache.discovered-inputs',
  'sec-17-new-4':          'exec.cache.test-unit',
  'sec-17-new-5':          'exec.cache.portability',
```

(Note: the parent slug `exec.cache` is preserved; only the child slugs are added.)

- [ ] **Step 4.E.3: Populate §17.2 from §8.6.1**

Slug `exec.cache.integrity` preserved.

- [ ] **Step 4.E.4: Populate §17.3 from §8.6.3**

Slug `exec.cache.discovered-inputs` preserved.

- [ ] **Step 4.E.5: Populate §17.4 from §8.6.4**

Slug `exec.cache.test-unit` preserved.

- [ ] **Step 4.E.6: Populate §17.5 from §7.7**

Read §7.7 of current `07-cross-cookfile-composition.mdx`. Copy. Update slug marker `[#modules.cache-invariants]` → `[#exec.cache.portability]`.

- [ ] **Step 4.E.7: Build, commit**

```bash
pnpm build
git add src/content/docs/17-cache.mdx scripts/slug-mapping.ts
git commit -m "docs(standard): create Ch. 17 Cache semantics (v0.10 reorg)"
```

### Task 4.F: Chapter 18 — Output materialisation

**Files:**
- Create: `src/content/docs/18-output-materialisation.mdx`

- [ ] **Step 4.F.1: Copy from §8.7**

```mdx
---
title: "§{exec.mat} — Output materialisation"
sidebar:
  order: 18
---
# 18. Output materialisation [#exec.mat]
> **Normative.** This chapter defines the parent-directory-creation rule that the engine performs before invoking a `cook` step's shell text.

## 18.1. Output path materialisation [#exec.output-materialisation]

<!-- §8.7 -->
```

Slug `exec.output-materialisation` preserved.

- [ ] **Step 4.F.2: Build, commit**

```bash
pnpm build
git add src/content/docs/18-output-materialisation.mdx
git commit -m "docs(standard): create Ch. 18 Output materialisation (v0.10 reorg)"
```

### Task 4.G: Chapter 19 — Diagnostic ordering

**Files:**
- Create: `src/content/docs/19-diagnostics.mdx`

- [ ] **Step 4.G.1: Copy from §8.8**

```mdx
---
title: "§{exec.diag} — Diagnostic ordering"
sidebar:
  order: 19
---
# 19. Diagnostic ordering [#exec.diag]
> **Normative.** This chapter defines the ordering relation between syntactic, semantic, and execute-time diagnostics that a conforming implementation MUST observe.

## 19.1. Diagnostic ordering [#exec.diagnostic-ordering]

<!-- §8.8 -->
```

Slug `exec.diagnostic-ordering` preserved.

- [ ] **Step 4.G.2: Build, commit**

```bash
pnpm build
git add src/content/docs/19-diagnostics.mdx
git commit -m "docs(standard): create Ch. 19 Diagnostic ordering (v0.10 reorg)"
```

### Task 4.H: Chapter 20 — Workspace root

**Files:**
- Create: `src/content/docs/20-workspace.mdx`

- [ ] **Step 4.H.1: Scaffold and populate from §7.6**

```mdx
---
title: "§{exec.ws} — Workspace root"
sidebar:
  order: 20
---
# 20. Workspace root [#exec.ws]
> **Normative.** This chapter defines the workspace-root determination algorithm. The workspace root anchors sigil-anchored (`//`) imports per §{comp.import} and bounds the project-root sandbox per §{lua.fs}.

## 20.1. Workspace root determination [#exec.ws.determination]

<!-- §7.6 -->
```

Read §7.6 of current `07-cross-cookfile-composition.mdx`. Copy. Update slug marker `[#modules.workspace-root]` → `[#exec.ws.determination]`. The "Definition: transitively imports" definition block stays inside §20.1.

- [ ] **Step 4.H.2: Build, commit**

```bash
pnpm build
git add src/content/docs/20-workspace.mdx
git commit -m "docs(standard): create Ch. 20 Workspace root (v0.10 reorg)"
```

---

## Task 5: Part III content (Chapters 21–26) — includes new normative prose

This is where the ~120 lines of new normative prose for §24 Both-phase API land. Other chapters in this part are reshuffles of current §6.

### Task 5.A: Chapter 21 — Surface overview

**Files:**
- Create: `src/content/docs/21-lua-api.mdx`

- [ ] **Step 5.A.1: Scaffold and populate**

```mdx
---
title: "§{lua} — Cook Lua API surface"
sidebar:
  order: 21
---
# 21. Cook Lua API surface [#lua]
> **Normative.** This chapter is the Cook Lua API's table of contents. The four phase-classified surfaces — register-phase (§{lua.reg}), execute-phase (§{lua.exe}), both-phase (§{lua.both}), and the filesystem/path helpers (§{lua.fs}, §{lua.path}) — are defined in the chapters that follow.

## 21.1. API surface overview [#lua.api-overview]

<!-- §6.1 -->

## 21.2. The `recipe` global [#lua.recipe-global]

<!-- §6.1.1 -->

## 21.3. The `env` alias inside config_block bodies [#lua.env-alias]

<!-- §6.1.2 -->
```

Populate from current §6.1, §6.1.1, §6.1.2.

- [ ] **Step 5.A.2: Update the API surface overview table**

In §21.1, the current table lists `cook`, `fs`, `path`, `recipe`, `inputs`, etc. Update the "Defined in" column to point at the new chapters:

| Global | Defined in |
|---|---|
| `cook` | §{lua.reg}, §{lua.exe}, §{lua.both} |
| `fs` | §{lua.fs} |
| `path` | §{lua.path} |
| `recipe` | §{lua.recipe-global} |
| `inputs` / `outputs` / `input` / `output` / `input_N` | §{lua.exe} |

- [ ] **Step 5.A.3: Build, commit**

```bash
pnpm build
git add src/content/docs/21-lua-api.mdx
git commit -m "docs(standard): create Ch. 21 Cook Lua API surface (v0.10 reorg)"
```

### Task 5.B: Chapter 22 — Register-phase API

**Files:**
- Create: `src/content/docs/22-register-phase.mdx`

Aggregates `cook.add_unit` (§6.2 + §6.2.1), `cook.exec`/`cook.interactive` (§6.3.2), `cook.recipe` (§6.3.3), `cook.add_test` (§6.3.5), and `cook.step_group` (currently mentioned but not formally defined — define it formally here).

- [ ] **Step 5.B.1: Scaffold**

```mdx
---
title: "§{lua.reg} — Register-phase API"
sidebar:
  order: 22
---
# 22. Register-phase API [#lua.reg]
> **Normative.** This chapter defines every Cook Lua API surface that is register-phase only. A call to any function in this chapter from execute-phase Lua MUST raise a Lua runtime error per §{rationale.execute-phase-api}.

## 22.1. `cook.add_unit` [#lua.add-unit]

<!-- §6.2 -->

### 22.1.1. `discovered_inputs` field [#lua.add-unit-discovered-inputs]

<!-- §6.2.1 — syntax only; semantics in §{exec.cache.discovered-inputs} -->

## 22.2. `cook.exec` and `cook.interactive` [#lua.cook-exec]

<!-- §6.3.2 -->

## 22.3. `cook.recipe` [#lua.cook-recipe]

<!-- §6.3.3 -->

## 22.4. `cook.add_test` [#lua.cook-add-test]

<!-- §6.3.5 -->

## 22.5. `cook.step_group` [#lua.step-group]

<!-- formalise from currently-scattered references -->
```

- [ ] **Step 5.B.2: Populate §22.1 + §22.1.1 from §6.2 + §6.2.1**

Copy. Renumber. Slugs `lua.add-unit` and `lua.add-unit-discovered-inputs` preserved.

In §22.1.1, the current §6.2.1 prose specifies BOTH the field syntax AND the semantic effect on cache. Trim the semantic-effect prose ("The semantic effect of `discovered_inputs` on the cache is specified in §{exec.cache.discovered-inputs}") to be a single cross-reference paragraph; the actual cache behaviour now lives only in §17.3.

- [ ] **Step 5.B.3: Populate §22.2 from §6.3.2**

Copy. Slug `lua.cook-exec` preserved.

- [ ] **Step 5.B.4: Populate §22.3 from §6.3.3**

Copy. Slug `lua.cook-recipe` preserved.

- [ ] **Step 5.B.5: Populate §22.4 from §6.3.5**

Copy. Slug `lua.cook-add-test` preserved.

- [ ] **Step 5.B.6: Write §22.5 `cook.step_group` (NEW NORMATIVE PROSE)**

Currently `cook.step_group` is mentioned in §6.2 Note 6.2.1 ("Adjacent `cook.add_unit` calls made inside the same register-phase `cook.step_group(fn)` invocation become siblings ...") and in §8.3 (step groups). It's not formally defined. Add a formal definition:

```mdx
## 22.5. `cook.step_group` [#lua.step-group]
**Signature.** `cook.step_group(fn: function) -> nil`
**Phase.** Register-phase only.

`cook.step_group` invokes `fn` synchronously on the register-phase Lua VM. Every `cook.add_unit` (§{lua.add-unit}) call made within the dynamic extent of `fn` records its unit as a member of a single **step group** (§{exec.step-groups}). Step-group members MAY execute in parallel during the execute phase.

`cook.step_group` MUST return after `fn` returns; nested `cook.step_group` calls within `fn` are unspecified.

A conforming implementation MUST raise a Lua runtime error when `cook.step_group` is called from execute-phase Lua. The diagnostic MUST identify the function name and the calling step kind.

### Example 22.5.1

```lua
cook.step_group(function()
    for _, source in ipairs(inputs) do
        cook.add_unit({
            inputs  = { source },
            output  = source:gsub("%.c$", ".o"),
            command = string.format("gcc -c %s -o %s", source, source:gsub("%.c$", ".o")),
        })
    end
end)
```

The N units registered inside the function body form one step group; the N compiles MAY run in parallel under the execute phase.

### Note 22.5.1

The surface `cook` step (§{steps.cook-single}, §{steps.cook-multi}) routes through `cook.step_group` implicitly. Authors of Cook modules (Part IV) typically call `cook.step_group` explicitly to record a fan-out workload.
```

Add the new slug to `scripts/slug-mapping.ts`:

```ts
  'sec-22-step-group':     'lua.step-group',
```

- [ ] **Step 5.B.7: Build, commit**

```bash
pnpm build
git add src/content/docs/22-register-phase.mdx scripts/slug-mapping.ts
git commit -m "$(cat <<'EOF'
docs(standard): create Ch. 22 Register-phase API (v0.10 reorg)

Includes new normative prose for §22.5 `cook.step_group`, previously
referenced but never formally defined.

EOF
)"
```

### Task 5.C: Chapter 23 — Execute-phase API

**Files:**
- Create: `src/content/docs/23-execute-phase.mdx`

- [ ] **Step 5.C.1: Scaffold and populate**

```mdx
---
title: "§{lua.exe} — Execute-phase API"
sidebar:
  order: 23
---
# 23. Execute-phase API [#lua.exe]
> **Normative.** This chapter defines the Cook Lua API surfaces that are only bound during the execute phase: the using-block globals on Lua-code work units, and the plate/test Lua-block bindings.

## 23.1. Using-block globals [#lua.using-block-globals]

<!-- §6.4 -->

## 23.2. Plate and test Lua-block bindings [#lua.using-block-globals-plate-test]

<!-- §6.4.1 -->
```

Populate from current §6.4 and §6.4.1. Slugs preserved.

- [ ] **Step 5.C.2: Build, commit**

```bash
pnpm build
git add src/content/docs/23-execute-phase.mdx
git commit -m "docs(standard): create Ch. 23 Execute-phase API (v0.10 reorg)"
```

### Task 5.D: Chapter 24 — Both-phase API (NEW NORMATIVE PROSE)

**Files:**
- Create: `src/content/docs/24-both-phase.mdx`

This chapter has the largest new normative content: ~120 lines covering surfaces previously specified only in CS amendments. Existing surfaces (`cook.sh`, `cook.load_module`) move here verbatim from §6.3.1 and §6.3.4.

- [ ] **Step 5.D.1: Scaffold**

```mdx
---
title: "§{lua.both} — Both-phase API"
sidebar:
  order: 24
---
# 24. Both-phase API [#lua.both]
> **Normative.** This chapter defines the Cook Lua API surfaces that are available in both register-phase and execute-phase Lua. The behavioural contract — signatures, error model, working-directory rooting — is identical across phases; only the surrounding scheduling differs.

## 24.1. `cook.sh` [#lua.cook-sh]

<!-- §6.3.1 verbatim -->

## 24.2. `cook.load_module` [#lua.cook-load-module]

<!-- §6.3.4 verbatim (lifecycle prose extracted to §12.3) -->

## 24.3. `cook.env` [#lua.cook-env]

<!-- NEW — see Step 5.D.4 -->

## 24.4. `cook.cache` [#lua.cook-cache]

<!-- NEW — see Step 5.D.5; pinned by CS-0070 -->

## 24.5. `cook.export` and `cook.import` [#lua.cook-export-import]

<!-- NEW — see Step 5.D.6; pinned by CS-0071 -->

## 24.6. `cook.platform` [#lua.cook-platform]

<!-- NEW — see Step 5.D.7 -->

## 24.7. `cook.dep_output` and `cook.dep_output_list` [#lua.cook-dep-output]

<!-- NEW — see Step 5.D.8 -->
```

- [ ] **Step 5.D.2: Populate §24.1 from §6.3.1**

Copy verbatim. Renumber. Slug `lua.cook-sh` preserved.

- [ ] **Step 5.D.3: Populate §24.2 from §6.3.4 minus lifecycle**

Copy current §6.3.4 verbatim, EXCEPT for the paragraphs that describe module lifecycle (top-level execution, `init()` call, per-VM caching, cycle detection). Those moved to §12.3. The §24.2 body retains: signature, phase classification, working-directory contract, cross-Cookfile resolution rule, error model, and the CS-0066/0069 cross-references (which apply to the API call surface, not lifecycle).

Add the slug to slug-mapping if not present:

```ts
  'sec-24-load-module':    'lua.cook-load-module',
```

(Note: the current `lua.cook-load-module` slug may already be present from earlier. Verify.)

- [ ] **Step 5.D.4: Write §24.3 `cook.env`**

```mdx
## 24.3. `cook.env` [#lua.cook-env]
**Type.** `table`
**Phase.** Both.

`cook.env` is the Cook-managed environment table. It is a regular Lua table with two semantic distinctions from a bare table:

- Keys MUST be strings and values MUST be strings; a conforming implementation MUST raise a Lua runtime error on a write whose key is not a string or whose value is not a string. (Numeric and boolean values are NOT auto-coerced; authors converting non-string values use Lua's `tostring`.)
- The table starts populated with the inherited process environment of the Cook invocation (e.g. `PATH`, `HOME`). Subsequent writes overlay this initial population.

`cook.env` is the source of `$<TOKEN>` env-var resolution (§{xref.resolution} step 3, §{xref.env-namespace}) and is merged onto the child-process environment for every shell unit (§{lua.cook-sh}, `cook.add_unit` shell payload). The implementation MUST track which keys of `cook.env` were consulted by each work unit (the `consulted_env_keys` set per §{xref.resolution}) for the purposes of cache invalidation (§{exec.cache.abstract}).

Within a `config_block` body (§{decl.config}), the bare global `env` is bound as an alias of `cook.env` per §{lua.env-alias}. Writes through either name MUST be observable through both.

### Example 24.3.1

```cook
config
    env.CC = "clang"
    env.CXXFLAGS = "-std=c++20 -O2"
```

The bare `env` inside the config body writes to `cook.env`; the keys `CC` and `CXXFLAGS` are subsequently resolvable as `$<CC>` and `$<CXXFLAGS>` placeholders in recipe bodies (§{phl.cook-step}).
```

- [ ] **Step 5.D.5: Write §24.4 `cook.cache`**

```mdx
## 24.4. `cook.cache` [#lua.cook-cache]
**Phase.** Both. [Pinned by CS-0070.]

`cook.cache` is the Cook-managed key/value store available to register-phase and execute-phase Lua. Its purpose is to let modules persist register-phase computations (compiler detection, package finder results) into a form that execute-phase code can read without redoing the work.

### 24.4.1. `cook.cache.get` [#lua.cook-cache-get]
**Signature.** `cook.cache.get(key: string) -> any`

Returns the value previously stored under `key` via `cook.cache.set` (or `nil` if no value is stored). The value MUST round-trip through any serialisation the implementation uses for persistence; non-serialisable values (Lua functions, userdata, coroutines) MUST raise a Lua runtime error when passed to `cook.cache.set`.

### 24.4.2. `cook.cache.set` [#lua.cook-cache-set]
**Signature.** `cook.cache.set(key: string, value: any) -> nil`

Stores `value` under `key`. Subsequent `cook.cache.get(key)` returns `value` (or its deserialisation-equal counterpart).

A register-phase write MUST be persisted by the implementation in a way that survives across Cook invocations: a register-phase `set` followed by a later register-phase `get` in a different Cook invocation MUST return the previously-stored value. The on-disk format is implementation-defined.

An execute-phase write MAY be in-memory-only, scoped to the worker VM's lifetime; a conforming implementation MAY discard execute-phase writes between invocations. Modules that rely on cross-invocation persistence MUST perform the persistent writes from register phase.

### 24.4.3. `cook.cache.scope` [#lua.cook-cache-scope]
**Signature.** `cook.cache.scope(label: string) -> table`

Returns a "scoped" table view whose `get(key)` and `set(key, value)` operations are equivalent to `cook.cache.get(label .. ":" .. key)` and `cook.cache.set(label .. ":" .. key, value)` respectively. The label string MUST NOT contain the `:` character; a conforming implementation MUST raise a Lua runtime error on such a label.

Scopes are conventionally named after the module that owns them (e.g. `cook_cc:toolchain`, `cook_cc:find:raylib`) to prevent key collisions between modules.
```

- [ ] **Step 5.D.6: Write §24.5 `cook.export` and `cook.import`**

```mdx
## 24.5. `cook.export` and `cook.import` [#lua.cook-export-import]
**Phase.** Both. [Pinned by CS-0071.]

`cook.export` and `cook.import` are the Cook-managed transitive-link-info channel used by target makers (e.g. `cc.bin`/`cc.lib` per §{cat.cc.transitive}) to publish a target's outward-facing fields and by consumers to read them.

### 24.5.1. `cook.export` [#lua.cook-export]
**Signature.** `cook.export(name: string, info: table) -> nil`

Records that the target named `name` exposes the fields in `info`. The `info` table's shape is conventionally established by the publisher's module documentation; for `cc` targets the conventional shape is documented in §{cat.cc.transitive}. The Standard does not constrain `info`'s shape beyond requiring it to be a Lua table.

A conforming implementation MUST persist the exported value in a way that subsequent `cook.import(name)` calls (in either phase, within the same Cookfile evaluation) return the exported value.

### 24.5.2. `cook.import` [#lua.cook-import]
**Signature.** `cook.import(name: string) -> table?`

Returns the table previously published for `name` via `cook.export`, or `nil` if no export has occurred.

A conforming implementation MUST resolve `cook.import` against the **same workspace's** export records — `cook.import` MUST NOT cross workspace boundaries. The interaction with cross-Cookfile composition is: a target maker invoked from an imported Cookfile (§{comp.import}) publishes exports against the workspace-global table, but resolution by recipe name in a different Cookfile uses the qualified name `alias.name` per §{comp.qualified-refs}.

The Cook implementation MAY share the export table between register-phase and execute-phase Lua VMs, or use a per-worker scratch store when the recipe body is both producer and consumer; the minimum bar is that the calls resolve without error and round-trip values within a single worker VM. Implementations that omit the execute-phase API break any target maker whose register-recorded body calls `cook.export` at execute time.
```

- [ ] **Step 5.D.7: Write §24.6 `cook.platform`**

```mdx
## 24.6. `cook.platform` [#lua.cook-platform]
**Type.** `table`
**Phase.** Both.

`cook.platform` is a Cook-managed read-only table exposing the host platform's identity. The following fields MUST be populated by a conforming implementation:

| Field | Type | Value |
|---|---|---|
| `os` | string | `"linux"`, `"darwin"`, `"windows"`, or `"unknown"`. |
| `arch` | string | `"x86_64"`, `"aarch64"`, `"arm"`, `"riscv64"`, or `"unknown"`. |
| `triple` | string | The host's compiler target triple, e.g. `"x86_64-unknown-linux-gnu"`, `"aarch64-apple-darwin"`. Implementation-defined when the host is not recognised by the implementation's target-triple resolver. |

The values MUST be identical between the register-phase Lua VM and every execute-phase worker VM within a single Cook invocation. Implementations realise this by sharing a single source-of-truth resolver across VMs; the reference implementation does so via `cook-lua-stdlib::platform`.

### Example 24.6.1

```lua
if cook.platform.os == "darwin" then
    cook.add_unit({
        command = "install_name_tool -id @rpath/libfoo.dylib build/lib/libfoo.dylib",
        cache   = false,
    })
end
```

The `cook.platform.os` check happens at register time on the register-phase VM; the unit is recorded only on macOS hosts.
```

- [ ] **Step 5.D.8: Write §24.7 `cook.dep_output` and `cook.dep_output_list`**

```mdx
## 24.7. `cook.dep_output` and `cook.dep_output_list` [#lua.cook-dep-output]
**Phase.** Both.

`cook.dep_output` and `cook.dep_output_list` are the Lua-side counterpart of the `$<NAME>` string-substitution surface (§{xref.string-substitution}). A `using >{ … }`, `plate >{ … }`, or `test >{ … }` body that needs to reference another recipe's output uses these functions instead of textual placeholder substitution.

### 24.7.1. `cook.dep_output` [#lua.cook-dep-output-single]
**Signature.** `cook.dep_output(name: string) -> string`

Returns the named recipe's output list joined by single space characters. Equivalent to the textual expansion of `$<NAME>` per §{xref.string-substitution}.

If the named recipe's output list is empty, returns the empty string; a conforming implementation MUST emit a warning at register time naming the referring recipe and the referent. The reference itself is not an error.

If `name` does not name a recipe in scope, a conforming implementation MUST raise a Lua runtime error at the calling step's register time.

### 24.7.2. `cook.dep_output_list` [#lua.cook-dep-output-list]
**Signature.** `cook.dep_output_list(name: string) -> table`

Returns the named recipe's output list as a 1-indexed Lua table of strings. Equivalent to `cook.dep_output(name)` but preserved as a sequence so that callers can iterate, index, or apply path accessors (§{xref.path-accessors}) per-element.

Same error model as `cook.dep_output`.

### Example 24.7.1

```cook
recipe libs
    cook "build/lib/libfoo.a" using { ar rcs $<out> ... }
    cook "build/lib/libbar.a" using { ar rcs $<out> ... }

recipe app
    cook "build/bin/app" using >{
        local libs = cook.dep_output_list("libs")
        local cmd = "gcc -o " .. output .. " main.c"
        for _, lib in ipairs(libs) do
            cmd = cmd .. " " .. lib
        end
        cook.sh(cmd)
    }
```

The `cook.dep_output_list("libs")` call returns `{ "build/lib/libfoo.a", "build/lib/libbar.a" }`. An equivalent shell-body recipe would use `$<libs>` per §{xref.string-substitution}.
```

- [ ] **Step 5.D.9: Add the new slugs to `scripts/slug-mapping.ts`**

```ts
  'sec-24':                'lua.both',
  'sec-24-1':              'lua.cook-sh',
  'sec-24-2':              'lua.cook-load-module',
  'sec-24-3':              'lua.cook-env',
  'sec-24-4':              'lua.cook-cache',
  'sec-24-4-1':            'lua.cook-cache-get',
  'sec-24-4-2':            'lua.cook-cache-set',
  'sec-24-4-3':            'lua.cook-cache-scope',
  'sec-24-5':              'lua.cook-export-import',
  'sec-24-5-1':            'lua.cook-export',
  'sec-24-5-2':            'lua.cook-import',
  'sec-24-6':              'lua.cook-platform',
  'sec-24-7':              'lua.cook-dep-output',
  'sec-24-7-1':            'lua.cook-dep-output-single',
  'sec-24-7-2':            'lua.cook-dep-output-list',
```

- [ ] **Step 5.D.10: Build, commit**

```bash
pnpm build
git add src/content/docs/24-both-phase.mdx scripts/slug-mapping.ts
git commit -m "$(cat <<'EOF'
docs(standard): create Ch. 24 Both-phase API (v0.10 reorg)

Includes ~120 lines of new normative prose formalising cook.env,
cook.cache, cook.export/import, cook.platform, and cook.dep_output
surfaces previously pinned only in CS-0066/0070/0071 amendments.

EOF
)"
```

### Task 5.E: Chapter 25 — `fs.*`

**Files:**
- Create: `src/content/docs/25-fs.mdx`

- [ ] **Step 5.E.1: Copy from §6.5**

```mdx
---
title: "§{lua.fs} — Filesystem helpers"
sidebar:
  order: 25
---
# 25. Filesystem helpers (`fs.*`) [#lua.fs]
> **Normative.** This chapter defines the `fs.*` table — the host-portable filesystem subset that Cook exposes in both register-phase and execute-phase Lua VMs — together with the project-root sandbox and the Lua-side shell escape hatch guards.

## 25.1. Overview [#lua.fs-helpers]

<!-- §6.5 lead-in paragraphs -->

## 25.2. `fs.exists` [#lua.fs-exists]

<!-- §6.5.1 -->

## 25.3. `fs.size` [#lua.fs-size]

<!-- §6.5.2 -->

## 25.4. `fs.read` [#lua.fs-read]

<!-- §6.5.3 -->

## 25.5. `fs.write` [#lua.fs-write]

<!-- §6.5.4 -->

## 25.6. `fs.mkdir_p` [#lua.fs-mkdir-p]

<!-- §6.5.5 -->

## 25.7. `fs.glob` [#lua.fs-glob]

<!-- §6.5.6 -->

## 25.8. `fs.mtime` [#lua.fs-mtime]

<!-- §6.5.7 -->

## 25.9. Project-root sandbox [#lua.fs-sandbox]

<!-- §6.5.8 -->

## 25.10. Lua-side shell escape hatches [#lua.shell-escape-hatches]

<!-- §6.5.9 -->
```

Populate each section from current `06-cook-lua-api.mdx`. Slugs preserved.

- [ ] **Step 5.E.2: Build, commit**

```bash
pnpm build
git add src/content/docs/25-fs.mdx
git commit -m "docs(standard): create Ch. 25 fs.* + sandbox + escape hatches (v0.10 reorg)"
```

### Task 5.F: Chapter 26 — `path.*`

**Files:**
- Create: `src/content/docs/26-path.mdx`

- [ ] **Step 5.F.1: Copy from §6.6**

```mdx
---
title: "§{lua.path} — Path helpers"
sidebar:
  order: 26
---
# 26. Path helpers (`path.*`) [#lua.path]
> **Normative.** This chapter defines the `path.*` table — pure string manipulation on path components, with no I/O.

## 26.1. Overview [#lua.path-helpers]

<!-- §6.6 lead-in -->

## 26.2. `path.stem` [#lua.path-stem]
## 26.3. `path.name` [#lua.path-name]
## 26.4. `path.ext` [#lua.path-ext]
## 26.5. `path.dir` [#lua.path-dir]
## 26.6. `path.replace_ext` [#lua.path-replace-ext]
## 26.7. `path.join` [#lua.path-join]
```

Populate from current §6.6. Slugs preserved.

- [ ] **Step 5.F.2: Build, commit**

```bash
pnpm build
git add src/content/docs/26-path.mdx
git commit -m "docs(standard): create Ch. 26 path.* (v0.10 reorg)"
```

---

## Task 6: Part IV content (Chapters 27–28)

### Task 6.A: Chapter 27 — Catalogue governance

**Files:**
- Create: `src/content/docs/27-catalogue.mdx`

- [ ] **Step 6.A.1: Scaffold and populate from §9.1**

```mdx
---
title: "§{cat} — Standard module catalogue"
sidebar:
  order: 27
---
# 27. Standard module catalogue [#cat]
> **Normative.** This chapter defines the catalogue of blessed Cook modules — modules curated by the Cook project and distributed through the official LuaRocks-backed registry (`rocks.usecook.com`). A module's presence as a numbered chapter in Part IV is what makes it "blessed": a conformance-checkable contract that any implementation of the module MUST honour.

The Standard governs the contract, not the reference implementation. The published rock is one implementation that meets the contract; a reimplementation in another language layer MUST produce semantically equivalent behaviour for every public surface specified here.

## 27.1. Bootstrap [#cat.bootstrap]

### 27.1.1. Install [#cat.bootstrap.install]

<!-- §9.1.1 -->

### 27.1.2. Vendoring escape hatch [#cat.bootstrap.vendor]

<!-- §9.1.2 -->

## 27.2. Catalogue index [#cat.index]

<!-- §9.1.3 -->
```

Populate each section from current §9.1.x.

- [ ] **Step 6.A.2: Build, commit**

```bash
pnpm build
git add src/content/docs/27-catalogue.mdx
git commit -m "docs(standard): create Ch. 27 Catalogue governance (v0.10 reorg)"
```

### Task 6.B: Chapter 28 — `cc` module

**Files:**
- Create: `src/content/docs/28-cc.mdx`

This is current §9.2 in its own file.

- [ ] **Step 6.B.1: Extract §9.2 into its own file**

```bash
# Copy current 09-standard-modules.mdx and trim to §9.2 only.
cp src/content/docs/09-standard-modules.mdx src/content/docs/28-cc.mdx
```

Open `src/content/docs/28-cc.mdx`:

1. Delete everything before `## 9.2. cc — C-family build module`.
2. Update frontmatter:

```yaml
---
title: "§{cat.cc} — cc — C-family build module"
sidebar:
  order: 28
---
```

3. Update the top-level heading from `## 9.2. cc — C-family build module` to `# 28. cc — C-family build module [#cat.cc]`.

4. Renumber every sub-section heading:

```bash
sed -i 's/^### 9\.2\.1\./## 28.1./g' src/content/docs/28-cc.mdx
sed -i 's/^### 9\.2\.2\./## 28.2./g' src/content/docs/28-cc.mdx
sed -i 's/^### 9\.2\.3\./## 28.3./g' src/content/docs/28-cc.mdx
sed -i 's/^#### 9\.2\.3\./### 28.3./g' src/content/docs/28-cc.mdx
sed -i 's/^##### 9\.2\.3\.8\.1\./#### 28.3.8.1./g' src/content/docs/28-cc.mdx
sed -i 's/^##### 9\.2\.3\.8\.2\./#### 28.3.8.2./g' src/content/docs/28-cc.mdx
sed -i 's/^##### 9\.2\.3\.8\.3\./#### 28.3.8.3./g' src/content/docs/28-cc.mdx
sed -i 's/^### 9\.2\.4\./## 28.4./g' src/content/docs/28-cc.mdx
sed -i 's/^### 9\.2\.5\./## 28.5./g' src/content/docs/28-cc.mdx
sed -i 's/^### 9\.2\.6\./## 28.6./g' src/content/docs/28-cc.mdx
```

Verify:

```bash
grep -E '^#+ [0-9]+' src/content/docs/28-cc.mdx
```

5. Update every `[#stdmods.cc.X]` slug marker to `[#cat.cc.X]`.

```bash
sed -i 's/\[#stdmods\.cc/[#cat.cc/g' src/content/docs/28-cc.mdx
```

6. Update slug refs within the file. The `cc` chapter currently references §6, §7, §9 etc. Use the rename table.

```bash
grep -nE '§\{(modules|grammar|stdmods|lua\.shell-placeholders)' src/content/docs/28-cc.mdx
```

For each hit, rewrite per Reference A.

- [ ] **Step 6.B.2: Build, commit**

```bash
pnpm build
git add src/content/docs/28-cc.mdx
git commit -m "docs(standard): create Ch. 28 cc module (v0.10 reorg)"
```

---

## Task 7: Annex reorganisation

**Files:**
- Rename: `appendix/B-rationale.mdx` → `appendix/C-rationale.mdx`
- Rename: `appendix/C-examples.mdx` → `appendix/B-examples.mdx`
- Rename: `appendix/D-changes.mdx` → `appendix/E-changes.mdx`
- Rename: `appendix/E-pre-v1-checklist.mdx` → `appendix/D-pre-v1-checklist.mdx`
- Create: `appendix/F-corpus.mdx` (stub)

- [ ] **Step 7.1: Swap Rationale and Examples**

```bash
git mv src/content/docs/appendix/B-rationale.mdx src/content/docs/appendix/temp.mdx
git mv src/content/docs/appendix/C-examples.mdx src/content/docs/appendix/B-examples.mdx
git mv src/content/docs/appendix/temp.mdx src/content/docs/appendix/C-rationale.mdx
```

Update frontmatter in each:

`appendix/B-examples.mdx`:

```yaml
---
title: "Appendix B — Worked examples"
sidebar:
  order: 11
---
# Appendix B. Worked examples (informative)
```

`appendix/C-rationale.mdx`:

```yaml
---
title: "Appendix C — Rationale"
sidebar:
  order: 12
---
# Appendix C. Rationale (informative)
```

- [ ] **Step 7.2: Swap Changes and Pre-v1 checklist**

```bash
git mv src/content/docs/appendix/D-changes.mdx src/content/docs/appendix/temp.mdx
git mv src/content/docs/appendix/E-pre-v1-checklist.mdx src/content/docs/appendix/D-pre-v1-checklist.mdx
git mv src/content/docs/appendix/temp.mdx src/content/docs/appendix/E-changes.mdx
```

Update frontmatter:

`appendix/D-pre-v1-checklist.mdx`:

```yaml
---
title: "Appendix D — Pre-1.0 checklist"
sidebar:
  order: 13
---
# Appendix D. Pre-1.0 checklist (informative)
```

`appendix/E-changes.mdx`:

```yaml
---
title: "Appendix E — Changes"
sidebar:
  order: 14
---
# Appendix E. Changes (informative)
```

- [ ] **Step 7.3: Rebase Rationale heading numbers (App. C)**

Open `appendix/C-rationale.mdx`. The current rationale entries use chapter-based section numbers (`B.0`, `B.2.2`, `B.3.9`, etc.). Under the new map:
- `B.0.x` (intro) → `C.0.x`
- `B.1.x` (notation) → `C.2.x` (Ch. 2 is notation)
- `B.2.x` (lexical) → `C.3.x`
- `B.3.x` (grammar) → `C.4.x`–`C.5.x`–`C.8.x` (the old grammar splits)
- `B.4.x` (recipes) → `C.6.x` + `C.7.x` + `C.8.x` (recipes + chores + step kinds)
- `B.5.x` (xref/exec) → `C.10.x` and `C.13–C.20`
- `B.6.x` (lua) → `C.21–C.26`
- `B.7.x` (modules) → `C.11.x` and `C.12.x`

Renumber every rationale entry's heading. Slug markers are preserved (`rationale.one-token-per-line` stays the same anchor; only the visible number changes).

This is mechanical but tedious. Use a script. For Cook Standard contributors who already know which CSes correspond to which rationale entries, the heading-number changes are:

```bash
# Renumber rationale headings — automated approach via sed substitution
# applied per heading line. The pattern matches "## B.N." and rewrites to
# "## C.<new>." per a mapping defined inline.

# B.0 → C.0  (intro)
sed -i 's/^## B\.0\./## C.0./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.0\./### C.0./g' src/content/docs/appendix/C-rationale.mdx

# B.1 → C.2  (notation)
sed -i 's/^## B\.1\./## C.2./g' src/content/docs/appendix/C-rationale.mdx

# B.2 → C.3  (lexical)
sed -i 's/^## B\.2\./## C.3./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.2\./### C.3./g' src/content/docs/appendix/C-rationale.mdx

# B.3 → C.4 (top-level), C.5 (declarations), C.8 (step dispatch).
# B.3 prose is split into three new chapters' rationale homes; for the
# initial cut, put ALL B.3 entries under C.4 (top-level) with notes
# directing readers to look at the relevant chapter. The clean split
# can be done in a follow-on editorial pass.
sed -i 's/^## B\.3\./## C.4./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.3\./### C.4./g' src/content/docs/appendix/C-rationale.mdx

# B.4 → C.6 (recipes); per the above note, leave B.4 chores and step-kind
# entries under C.6 too in the initial cut.
sed -i 's/^## B\.4\./## C.6./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.4\./### C.6./g' src/content/docs/appendix/C-rationale.mdx

# B.5 → C.10 (xref) and C.13 onward (exec); collapse to C.13 for the
# initial cut.
sed -i 's/^## B\.5\./## C.13./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.5\./### C.13./g' src/content/docs/appendix/C-rationale.mdx

# B.6 → C.21 (Lua API surface)
sed -i 's/^## B\.6\./## C.21./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.6\./### C.21./g' src/content/docs/appendix/C-rationale.mdx

# B.7 → C.11 (composition; some entries belong in C.12)
sed -i 's/^## B\.7\./## C.11./g' src/content/docs/appendix/C-rationale.mdx
sed -i 's/^### B\.7\./### C.11./g' src/content/docs/appendix/C-rationale.mdx
```

Slugs (`rationale.X`) on each heading line are preserved by the substitution above (sed only touches the leading `## B.N.` prefix).

Verify slugs are intact:

```bash
grep -E '\[#rationale\.' src/content/docs/appendix/C-rationale.mdx | wc -l
```

Expected: same count as before the sed run (sed didn't touch the `[#slug]` markers, only the numeric prefix).

- [ ] **Step 7.4: Add per-version subheaders to Changes (App. E)**

Open `appendix/E-changes.mdx`. The current "## Versions" index at the top is preserved. For each `## CS-XXXX` entry below, prepend a `### vY.Z` H3 above the first CS entry of that version. This is a readability improvement; structurally, the file gets ~9 new headings (one per version v0.1 through v0.9 plus v0.10).

Example:

```mdx
## Versions

(existing versions index)

### v0.1

## CS-0001 — Cook Standard v0.1 established

...

## CS-0002 — Planned: tree-sitter-cook conformance audit

...

(more v0.1 CSes ...)

### v0.2

## CS-0011 — VarDecl removal

...
```

(The double `## CS-XXXX` H2 inside an H3 section is intentional — readers' table-of-contents view stays usable.)

Add slugs to the new version subheaders if desired:

```mdx
### v0.1 [#changes.v0-1]
```

- [ ] **Step 7.5: Add `pre-v1.*` slugs to checklist**

Open `appendix/D-pre-v1-checklist.mdx`. Each `## E.X.` heading currently has no slug marker. Add slugs:

```bash
# Add [#pre-v1.parse-txt-coupling] after the existing E.1 heading text, etc.
# Manual edit each heading. The conversion is:
#   ## E.1. Conformance corpus is parser-impl-coupled
#   → ## D.1. Conformance corpus is parser-impl-coupled [#pre-v1.parse-txt-coupling]
```

Renumber `## E.` → `## D.`:

```bash
sed -i 's/^# Appendix E\./# Appendix D./' src/content/docs/appendix/D-pre-v1-checklist.mdx
sed -i 's/^## E\./## D./g' src/content/docs/appendix/D-pre-v1-checklist.mdx
sed -i 's/^### E\./### D./g' src/content/docs/appendix/D-pre-v1-checklist.mdx
```

Add `[#pre-v1.X]` markers to each numbered section. The slugs to use match the entries already added to `scripts/slug-mapping.ts` in Step 1.3.

- [ ] **Step 7.6: Create `appendix/F-corpus.mdx` stub**

```mdx
---
title: "Appendix F — Conformance corpus"
sidebar:
  order: 15
---
# Appendix F. Conformance corpus (informative)
> **Informative.** This appendix is a placeholder for the embedded conformance corpus. The current corpus lives on disk under `standard/conformance/positive/` and `standard/conformance/negative/`. A future Cook Standard cut will inline the corpus into this appendix so the Standard renders as a self-contained reference.

## F.1. Status [#corpus.status]

The corpus is consumed by `cook-lang`'s test suite (`cargo test -p cook-lang --test conformance`) and is referenced by the conformance criteria in §{conf.criteria}. Implementations seeking conformance should clone the repository and run the harness against their parser; this appendix will be expanded to a rendered enumeration in a future cut.
```

- [ ] **Step 7.7: Build, commit**

```bash
pnpm build
git add src/content/docs/appendix/
git commit -m "$(cat <<'EOF'
docs(standard): reorganise annexes for v0.10 reorg

- App. B/C swap (Examples before Rationale)
- App. D/E swap (Pre-1.0 checklist before Changes)
- App. C heading numbers renumbered to new chapter map
- App. E gets per-version subheaders
- New App. F stub for embedded conformance corpus
- pre-v1.* slugs added to Pre-1.0 checklist entries

EOF
)"
```

---

## Task 8: Delete legacy files; finalise sidebar; flip strict mode

**Files:**
- Delete: `src/content/docs/01-notation.mdx`, `02-lexical.mdx`, `03-syntactic-grammar.mdx`, `04-recipes.mdx`, `04a-chores.mdx`, `05-cross-recipe-references.mdx`, `06-cook-lua-api.mdx`, `07-cross-cookfile-composition.mdx`, `08-execution-model.mdx`, `09-standard-modules.mdx`
- Modify: `astro.config.mjs` (sidebar)
- Modify: `scripts/slug-mapping.ts` (remove retired entries)
- Modify: `src/plugins/__tests__/no-retired-slugs-in-source.test.ts` (un-skip)
- Modify: `src/plugins/__tests__/slug-renames-consistency.test.ts` (un-skip second test)

- [ ] **Step 8.1: Delete legacy MDX files**

```bash
git rm src/content/docs/01-notation.mdx \
       src/content/docs/02-lexical.mdx \
       src/content/docs/03-syntactic-grammar.mdx \
       src/content/docs/04-recipes.mdx \
       src/content/docs/04a-chores.mdx \
       src/content/docs/05-cross-recipe-references.mdx \
       src/content/docs/06-cook-lua-api.mdx \
       src/content/docs/07-cross-cookfile-composition.mdx \
       src/content/docs/08-execution-model.mdx \
       src/content/docs/09-standard-modules.mdx
```

- [ ] **Step 8.2: Update `astro.config.mjs` sidebar**

Replace the existing `sidebar:` block in `astro.config.mjs` with:

```js
      sidebar: [
        { label: 'Overview', link: '/' },
        {
          label: 'Front matter',
          items: [
            { label: '§ 0 — Introduction', link: '/00-introduction/' },
            { label: '§ 1 — Conformance',  link: '/01-conformance/' },
            { label: '§ 2 — Notation',     link: '/02-notation/' },
          ],
        },
        {
          label: 'Part I — The Cookfile language',
          items: [
            { label: '§ 3 — Lexical structure',          link: '/03-lexical/' },
            { label: '§ 4 — Top-level structure',        link: '/04-toplevel/' },
            { label: '§ 5 — Declarations',               link: '/05-declarations/' },
            { label: '§ 6 — Recipes',                    link: '/06-recipes/' },
            { label: '§ 7 — Chores',                     link: '/07-chores/' },
            { label: '§ 8 — Step kinds',                 link: '/08-step-kinds/' },
            { label: '§ 9 — Placeholders',               link: '/09-placeholders/' },
            { label: '§ 10 — Cross-recipe references',   link: '/10-cross-recipe-references/' },
            { label: '§ 11 — Cross-Cookfile composition',link: '/11-cross-cookfile-composition/' },
            { label: '§ 12 — Modules',                   link: '/12-modules/' },
          ],
        },
        {
          label: 'Part II — Execution model',
          items: [
            { label: '§ 13 — Two-phase model',           link: '/13-two-phase/' },
            { label: '§ 14 — Capture mode',              link: '/14-capture-mode/' },
            { label: '§ 15 — Step groups',               link: '/15-step-groups/' },
            { label: '§ 16 — Ordering & drain',          link: '/16-ordering-drain/' },
            { label: '§ 17 — Cache semantics',           link: '/17-cache/' },
            { label: '§ 18 — Output materialisation',    link: '/18-output-materialisation/' },
            { label: '§ 19 — Diagnostic ordering',       link: '/19-diagnostics/' },
            { label: '§ 20 — Workspace root',            link: '/20-workspace/' },
          ],
        },
        {
          label: 'Part III — The Cook Lua API',
          items: [
            { label: '§ 21 — API surface overview',      link: '/21-lua-api/' },
            { label: '§ 22 — Register-phase API',        link: '/22-register-phase/' },
            { label: '§ 23 — Execute-phase API',         link: '/23-execute-phase/' },
            { label: '§ 24 — Both-phase API',            link: '/24-both-phase/' },
            { label: '§ 25 — fs.* (incl. sandbox)',      link: '/25-fs/' },
            { label: '§ 26 — path.*',                    link: '/26-path/' },
          ],
        },
        {
          label: 'Part IV — Standard module catalogue',
          items: [
            { label: '§ 27 — Catalogue governance',      link: '/27-catalogue/' },
            { label: '§ 28 — cc — C-family build module',link: '/28-cc/' },
          ],
        },
        {
          label: 'Annexes',
          collapsed: true,
          items: [
            { label: 'Appendix A — Grammar',           link: '/appendix/a-grammar/' },
            { label: 'Appendix B — Worked examples',   link: '/appendix/b-examples/' },
            { label: 'Appendix C — Rationale',         link: '/appendix/c-rationale/' },
            { label: 'Appendix D — Pre-1.0 checklist', link: '/appendix/d-pre-v1-checklist/' },
            { label: 'Appendix E — Changes',           link: '/appendix/e-changes/' },
            { label: 'Appendix F — Conformance corpus',link: '/appendix/f-corpus/' },
          ],
        },
      ],
```

- [ ] **Step 8.3: Finalise `scripts/slug-mapping.ts`**

Remove every entry whose value is a retired slug (those starting with `grammar.`, `modules.`, `stdmods.`, plus `lexical.placeholders`, `lua.shell-placeholders`, `lua.shell-placeholders-plate-test`, `lua.use-env`, `lua.builtin-modules`, `lua.local-modules`, `recipes.body-bundling`).

Replace the `sec-*-new` placeholder keys added in Step 1.3 with proper `sec-N-M-K` keys matching the new chapter numbers. For example:

```ts
  // Ch. 4 — Top-level structure
  'sec-4':       'toplevel',
  'sec-4-1':     'toplevel.overview',
  'sec-4-2':     'toplevel.ordering',
  'sec-4-3':     'toplevel.termination',
  'sec-4-4':     'toplevel.module-call',
```

Audit every chapter's slugs against the new headings:

```bash
grep -E '^#+ [0-9]+.*\[#[a-z]' src/content/docs/**/*.mdx
```

- [ ] **Step 8.4: Un-skip the source-lint test**

Open `src/plugins/__tests__/no-retired-slugs-in-source.test.ts`. Remove the `.skip` so the test runs:

```ts
  it('no §{retired-slug} in any rendered source', () => {
```

Open `src/plugins/__tests__/slug-renames-consistency.test.ts`. Remove the `.skip` on the second test:

```ts
  it('no retired slug is itself a living slug', () => {
```

Run both:

```bash
pnpm test
```

Expected: all tests PASS. If the source-lint test fails, fix the offending source files (the test prints offending file:slug pairs).

- [ ] **Step 8.5: Verify build is fully clean**

```bash
pnpm build 2>&1 | tee build.log
grep -iE 'warn|error' build.log | grep -v 'astro:.*ready'
```

Expected: zero warnings/errors related to slug resolution, redirects, or missing pages. Astro's "ready" / "build complete" messages may appear; ignore them.

- [ ] **Step 8.6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs(standard): delete legacy MDX, finalise sidebar, flip strict slug lint (v0.10 reorg)

- Removes 01-notation.mdx through 09-standard-modules.mdx and 04a-chores.mdx
- Updates astro.config.mjs sidebar to four Parts + annexes
- Removes retired slug entries from slug-mapping.ts
- Activates the source-lint test (no retired slugs in source)
- Activates the consistency test (no retired slug masquerades as living)

EOF
)"
```

---

## Task 9: Validation gauntlet, CS entry, version bump, tag

**Files:**
- Modify: `appendix/E-changes.mdx` (new CS entry)
- Modify: `VERSION`

- [ ] **Step 9.1: Run the full validation gauntlet**

```bash
pnpm test
```

Expected: all tests PASS.

```bash
pnpm build
```

Expected: SUCCEEDS with no warnings related to slugs, redirects, or missing pages.

```bash
cd .. && cargo test -p cook-lang --test conformance && cd standard
```

Expected: conformance harness PASSES. (The Standard's content reorg doesn't touch the corpus.)

```bash
cook standard.lint
```

(Run from the worktree root.) Expected: PASS. If the binary isn't available in PATH within the worktree, run:

```bash
cd .. && cook standard.lint && cd standard
```

```bash
cd .. && cook standard.against-tag cs-standard/v0.9 && cd standard
```

Expected: per App. D (was E) §D.1 (was E.1), this MAY fail due to parser-impl drift. Document the result. If it fails, note in the CS entry.

- [ ] **Step 9.2: Manual redirect smoke test**

Start the dev server in another shell:

```bash
pnpm dev &
```

Visit the following URLs and verify each lands at the indicated new chapter:

| Old URL | Should redirect to |
|---|---|
| `http://localhost:4321/03-syntactic-grammar/` | `/04-toplevel/` |
| `http://localhost:4321/04-recipes/` | `/06-recipes/` |
| `http://localhost:4321/04a-chores/` | `/07-chores/` |
| `http://localhost:4321/05-cross-recipe-references/` | `/10-cross-recipe-references/` |
| `http://localhost:4321/06-cook-lua-api/` | `/21-lua-api/` |
| `http://localhost:4321/07-cross-cookfile-composition/` | `/11-cross-cookfile-composition/` |
| `http://localhost:4321/08-execution-model/` | `/13-two-phase/` |
| `http://localhost:4321/09-standard-modules/` | `/27-catalogue/` |
| `http://localhost:4321/appendix/b-rationale/` | `/appendix/c-rationale/` |
| `http://localhost:4321/appendix/d-changes/` | `/appendix/e-changes/` |

Kill the dev server:

```bash
kill %1
```

- [ ] **Step 9.3: Verify zero retired slugs in source**

```bash
grep -rE '§\{(grammar|modules|stdmods)\.' src/content/docs/ | grep -v 'slug-renames'
grep -rE '\[#(grammar|modules|stdmods)\.' src/content/docs/ | grep -v 'slug-renames'
grep -rE '§\{lexical\.placeholders\}' src/content/docs/
grep -rE '§\{lua\.shell-placeholders' src/content/docs/
grep -rE '§\{lua\.use-env\}' src/content/docs/
grep -rE '§\{recipes\.body-bundling\}' src/content/docs/
```

Expected: zero hits from each command.

- [ ] **Step 9.4: Write the CS entry**

Open `appendix/E-changes.mdx`. Identify the next available CS number by looking at the most recent entry:

```bash
grep -E '^## CS-[0-9]+' src/content/docs/appendix/E-changes.mdx | tail -3
```

The next number is one greater than the highest existing CS number (likely CS-0073 if CS-0072 was the last cut). Use that.

Insert the CS entry just below the "## Versions" index and above the first per-version subheader (so chronological order is preserved):

```mdx
## CS-NNNN — Structural redesign: Parts and per-topic chapters

**Date:** 2026-05-13
**Version:** v0.10
**Sections affected:** entire Standard (reorganisation). Specifically:
  - Renumbered Chapters 0–9 → Chapters 0–28 across four Parts.
  - Retired slug prefixes: `grammar.*`, `modules.*`, `stdmods.*`,
    `lexical.placeholders`, `lua.shell-placeholders`, `lua.use-env`,
    `lua.builtin-modules`, `lua.local-modules`, `recipes.body-bundling`.
  - New slug prefixes: `conf.*`, `toplevel.*`, `decl.*`, `steps.*`,
    `phl.*`, `comp.*`, `mods.*`, `cat.*`, `pre-v1.*`, `corpus.*`.
  - New normative prose:
    - §22.5 `cook.step_group` (formal definition);
    - §24.3 `cook.env`, §24.4 `cook.cache`, §24.5 `cook.export`/`cook.import`,
      §24.6 `cook.platform`, §24.7 `cook.dep_output`/`cook.dep_output_list`
      (formalises surfaces previously pinned in CS-0066/0070/0071 amendments);
    - §12.3 Module lifecycle (consolidates lifecycle prose previously
      scattered across §6.3.4, §6.8, and §7.4).
  - Annexes B and C swapped (Examples now precedes Rationale).
  - Annexes D and E swapped (Pre-1.0 checklist now precedes Changes).
  - Annex C (Rationale) heading numbers renumbered to match new chapter
    map; slugs preserved.
  - Annex E (Changes) gets per-version subheaders.
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
  groups. Step dispatch has one normative source. The new
  normative prose in §22.5, §24.3–§24.7, and §12.3 formalises
  Cook Lua surfaces previously specified only in amendments.
  No normative behaviour changes for any conforming Cookfile;
  this entry is structural and editorial only. The conformance
  corpus is unchanged. Retired anchored URLs continue to resolve
  through build-emitted page-level redirects (anchor-level
  precision for retired anchors is implemented via the
  client-side fragment rewriter in a follow-on editorial pass).

**Reference:** this commit.
```

(Replace `NNNN` with the actual next-available CS number.)

- [ ] **Step 9.5: Bump VERSION**

Open `VERSION`:

```
0.10
```

Replace `0.9` with `0.10`.

- [ ] **Step 9.6: Final build sanity check**

```bash
pnpm build
```

Expected: SUCCEEDS. The VERSION update means the rendered site header now shows v0.10.

- [ ] **Step 9.7: Commit**

```bash
git add src/content/docs/appendix/E-changes.mdx VERSION
git commit -m "$(cat <<'EOF'
docs(standard): cut Cook Standard v0.10 — structural redesign (CS-NNNN)

See CS-NNNN in Appendix E for the full sections-affected list.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

(Replace `NNNN` with the actual CS number used in Step 9.4.)

- [ ] **Step 9.8: Tag the cut**

```bash
git tag cs-standard/v0.10
```

- [ ] **Step 9.9: Final acceptance checklist**

Confirm all of the following before opening the PR:

- [ ] All seven validation gates from the design spec §9 passed.
- [ ] Every retired slug from `slug-renames.ts` has a redirect target that resolves.
- [ ] The git history contains nine logical commits (one per Task).
- [ ] The CS-NNNN entry is in `E-changes.mdx` and the `VERSION` bump to `0.10` is in the same commit.
- [ ] The pre-commit hook passes (run `git status` — no warnings about standard-not-updated).
- [ ] `cook standard.lint` reports zero new RFC 2119 violations.
- [ ] A reviewer can navigate from the rendered TOC to every numbered section and from every numbered section back to its annexed rationale.
- [ ] The tag `cs-standard/v0.10` exists locally.

- [ ] **Step 9.10: Open the PR**

```bash
git push -u origin standard-v0.10-reorg
gh pr create --title "docs(standard): cut Cook Standard v0.10 — structural redesign" --body "$(cat <<'EOF'
## Summary

- Reorganises the Cook Standard into four Parts with per-topic chapters per the approved design at `standard/specs/2026-05-13-standard-reorg-design.md`.
- Adds ~160 lines of new normative prose formalising Cook Lua surfaces previously pinned only in CS-0066/0070/0071 amendments (§12.3, §22.5, §24.3–§24.7).
- Renames retired slug prefixes (`grammar.*`, `modules.*`, `stdmods.*`, several `lua.*` and `lexical.*` leaves) and adds build-emitted page-level redirects so old URLs continue to resolve.
- No normative behaviour changes for any conforming Cookfile; the conformance corpus is unchanged.

## Test plan
- [ ] `pnpm test` (all gates green)
- [ ] `pnpm build` (no slug/redirect warnings)
- [ ] `cargo test -p cook-lang --test conformance` (corpus contract intact)
- [ ] `cook standard.lint` (RFC 2119 clean)
- [ ] Manual: ten retired-URL smoke tests per design spec §9c

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Final self-review checklist

Run through this before declaring the plan complete. Fix issues inline.

- [ ] **Spec coverage:** Every numbered section of `standard/specs/2026-05-13-standard-reorg-design.md` is implemented by a task. Cross-Part moves (§5.1 of the spec) all appear in the migration map. New normative prose (§6.1, §6.2, §6.3 of the spec) is fully written out in this plan, not deferred.

- [ ] **Placeholder scan:** No "TBD", "TODO", "fill in later" in any task. Each step has either the exact content to write or the exact bash/sed command.

- [ ] **Type consistency:** Slug names used in Task 1's rename table match slug names used in Tasks 3–7's heading markers. Spot-checked: `phl.token` (Task 1) appears in Task 3.G as the §9.1 slug marker. `exec.body-bundling` (Task 1) appears in Task 4.C as the §15.2 slug marker. `cat.cc.*` slugs in Task 1 match Task 6.B's substitution rules.

- [ ] **Slug coverage:** Every retired slug in Reference A.1 has a `[#new-slug]` heading marker assignment in the relevant task. Spot-checked: `grammar.step-dispatch` → `steps.dispatch` lands in Task 3.F. `modules.workspace-root` → `exec.ws.determination` lands in Task 4.H.

- [ ] **Commit message format:** All sample commit messages follow `docs(standard): ...` convention from the existing repo. Multi-line commits use HEREDOC pattern per CLAUDE.md guidance.

- [ ] **Worktree paths:** Step 1.x onward assumes the worktree at `/home/alex/dev/cook-standard-reorg`. Every `pnpm` and `cargo` invocation has the implicit working directory documented.
