# Slug-based cross-references — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace positional `§ N.M` cross-references with author-assigned stable slugs so section renumbers never break links.

**Architecture:** Each numbered heading gets an explicit slug via a trailing `[#chapter.leaf]` marker. Prose refs use `§{chapter.leaf}` syntax. Four rehype/remark plugins are rewritten or added; one migration script and one mapping table land in-tree. Readers still see `§ N.M` — the numeric is pulled live from headings at build time.

**Tech Stack:** TypeScript, Astro 5 + Starlight, unified/remark/rehype, vitest.

**Reference:** `docs/superpowers/specs/2026-04-24-standard-slug-based-refs-design.md`

---

## File Structure

**Files created:**
- `standard/src/plugins/rehype-bare-ref-lint.ts` — fails build on bare `§ N.M` in prose
- `standard/scripts/slug-mapping.ts` — hand-authored `sec-N-M-K → slug` table; the authoritative slug registry
- `standard/scripts/migrate-slugs.mjs` — one-shot script; rewrites headings and refs across `src/content/docs/`
- `standard/src/plugins/__tests__/rehype-bare-ref-lint.test.ts`

**Files modified:**
- `standard/src/plugins/clauses.ts` — harvests by slug instead of `sec-N-M-K`; emits the live numeric text for render-time substitution
- `standard/src/plugins/rehype-clause-anchors.ts` — reads the `[#slug]` marker; validates; applies as heading id
- `standard/src/plugins/rehype-clause-xrefs.ts` — new regex `§\{slug\}`; hard-fails on unresolved
- `standard/astro.config.mjs` — wires `rehype-bare-ref-lint` into `rehypePlugins`
- `standard/src/plugins/__tests__/rehype-clause-anchors.test.ts`
- `standard/src/plugins/__tests__/rehype-clause-xrefs.test.ts`
- `standard/src/content/docs/01-notation.mdx` — rewrites § 1.2 and § 1.7
- `standard/src/content/docs/appendix/D-changes.mdx` — adds CS-NNNN entry
- All 13 `.mdx` files under `standard/src/content/docs/` — migrated by script

**Heading marker syntax:** `## 2.3. Identifiers. [#lexical.identifiers]`. The bracket-hash form survives MDX parsing (unresolved shortcut reference link → literal text in hast) and is trivial for the anchors plugin to match and strip.

---

## Task 1: Slug harvester and types in `clauses.ts`

**Files:**
- Modify: `standard/src/plugins/clauses.ts` (full rewrite; keep exported symbol names)
- Test: indirect via Task 2/3 tests; no new `clauses.test.ts`

- [ ] **Step 1: Rewrite `clauses.ts`**

```typescript
import fs from 'node:fs';
import path from 'node:path';

export interface ClauseInfo {
  // Absolute site-relative URL including the fragment, e.g.
  // "/02-lexical/#lexical.identifiers".
  href: string;
  // The heading's live number, used as rendered link text
  // (e.g. "2.3", "A.4", "5"). Recomputed on every build.
  number: string;
  // The heading's visible text minus the number and the [#slug] marker,
  // used as the xref's `title` attribute. E.g. "Identifiers".
  text: string;
}

// Matches clause-numbered heading text:
//   <NUM>. <TITLE>. [#<slug>]
// where NUM is a digit run or single uppercase letter (plus optional .M[.K]),
// TITLE is any run up to the [#, and slug matches the slug grammar.
// Capture groups:
//   1 = top   (digit run or letter)
//   2 = mid   (digits, optional)
//   3 = bot   (digits, optional)
//   4 = title (non-greedy)
//   5 = slug  (chapter.leaf grammar)
const HEADING_RE =
  /^(?:#+)\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\.\s+(.+?)\s+\[#([a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)?)\]\s*$/gm;

function numberFrom(top: string, mid?: string, bot?: string): string {
  return [top, mid, bot].filter(Boolean).join('.');
}

// Starlight lowercases file slugs. `02-lexical.mdx` → `/02-lexical/`;
// `appendix/A-grammar.mdx` → `/appendix/a-grammar/`.
function fileToRoute(relPath: string): string {
  const noExt = relPath.replace(/\.mdx$/, '').toLowerCase();
  if (noExt === 'index') return '/';
  return `/${noExt}/`;
}

function walkMdx(root: string, out: string[]): void {
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const p = path.join(root, entry.name);
    if (entry.isDirectory()) walkMdx(p, out);
    else if (entry.isFile() && entry.name.endsWith('.mdx')) out.push(p);
  }
}

/**
 * Harvests every clause-numbered heading and returns a map from slug
 * (e.g. "lexical.identifiers") to the cross-file route + live number +
 * title. Consumed by rehype-clause-xrefs at build time.
 *
 * Throws on:
 * - Duplicate slug across any two headings.
 * - Any clause-numbered heading that lacks a [#slug] marker.
 */
export function harvestClauses(contentRoot: string): Map<string, ClauseInfo> {
  const map = new Map<string, ClauseInfo>();
  const seenAt = new Map<string, string>();

  // First pass: collect slugged headings.
  const files: string[] = [];
  walkMdx(contentRoot, files);

  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const route = fileToRoute(rel);
    const src = fs.readFileSync(abs, 'utf8');

    for (const m of src.matchAll(HEADING_RE)) {
      const [, top, mid, bot, title, slug] = m;
      const number = numberFrom(top, mid, bot);
      if (seenAt.has(slug)) {
        throw new Error(
          `duplicate slug "${slug}": first seen at ${seenAt.get(slug)}, also at ${rel}`,
        );
      }
      seenAt.set(slug, rel);
      map.set(slug, {
        href: `${route}#${slug}`,
        number,
        text: title.trim(),
      });
    }
  }

  // Second pass: detect numbered headings missing a slug marker.
  // A heading line starts with #+ space, then clause grammar, then period.
  // If it lacks `[#...]` trailing, that's a build error.
  const numberedNoSlug = /^#+\s+(?:[0-9]+|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\.\s+(.+)$/gm;
  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const src = fs.readFileSync(abs, 'utf8');
    for (const m of src.matchAll(numberedNoSlug)) {
      const rest = m[1];
      if (!/\[#[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)?\]\s*$/.test(rest)) {
        throw new Error(
          `numbered heading without [#slug] marker in ${rel}: "${m[0]}"`,
        );
      }
    }
  }

  return map;
}

export function defaultContentRoot(projectRoot: string): string {
  return path.join(projectRoot, 'src/content/docs');
}
```

- [ ] **Step 2: Commit**

```bash
git add standard/src/plugins/clauses.ts
git commit -m "feat(standard): harvest clauses by slug instead of sec-N-M-K

Introduces the slug-based clause registry. ClauseInfo now stores the
live numeric (pulled from the heading) and the title, keyed by the
author-assigned slug. The harvester also fails loud on missing [#slug]
markers and duplicate slugs."
```

---

## Task 2: Rewrite `rehype-clause-anchors.ts`

**Files:**
- Modify: `standard/src/plugins/rehype-clause-anchors.ts`
- Test: `standard/src/plugins/__tests__/rehype-clause-anchors.test.ts`

- [ ] **Step 1: Replace the existing test file**

```typescript
import { describe, it, expect } from 'vitest';
import { rehype } from 'rehype';
import { rehypeClauseAnchors } from '../rehype-clause-anchors';

function process(html: string): string {
  return String(
    rehype()
      .data('settings', { fragment: true })
      .use(rehypeClauseAnchors)
      .processSync(html),
  );
}

describe('rehypeClauseAnchors', () => {
  it('reads [#slug] from a two-level clause heading', () => {
    const out = process('<h2>2.3. Identifiers. [#lexical.identifiers]</h2>');
    expect(out).toContain('id="lexical.identifiers"');
    // Marker is stripped from the rendered heading text.
    expect(out).not.toContain('[#lexical.identifiers]');
    expect(out).toContain('>2.3. Identifiers.</h2>');
  });

  it('reads [#slug] from a three-level clause heading', () => {
    const out = process(
      '<h3>4.6.2. Multi-output cook. [#recipes.multi-output-cook]</h3>',
    );
    expect(out).toContain('id="recipes.multi-output-cook"');
  });

  it('reads [#slug] from an appendix clause heading', () => {
    const out = process('<h2>A.3. Top level. [#grammar-appendix.top-level]</h2>');
    expect(out).toContain('id="grammar-appendix.top-level"');
  });

  it('reads [#slug] from a chapter-level heading', () => {
    const out = process('<h1>2. Lexical structure. [#lexical]</h1>');
    expect(out).toContain('id="lexical"');
  });

  it('throws on a numbered heading missing the [#slug] marker', () => {
    expect(() => process('<h2>2.3. Identifiers.</h2>')).toThrowError(
      /missing \[#slug\] marker/i,
    );
  });

  it('throws on a slug that violates the grammar', () => {
    expect(() =>
      process('<h2>2.3. Identifiers. [#Bad_Slug]</h2>'),
    ).toThrowError(/invalid slug/i);
  });

  it('throws on duplicate slugs within a single build', () => {
    const input =
      '<h2>2.3. A. [#lexical.identifiers]</h2>' +
      '<h2>2.4. B. [#lexical.identifiers]</h2>';
    expect(() => process(input)).toThrowError(/duplicate slug/i);
  });

  it('ignores "Note N.M" subheadings (no clause number prefix)', () => {
    const out = process('<h3>Note 2.1.1</h3>');
    // No clause prefix, so anchors plugin skips it. Remains id-less.
    expect(out).not.toMatch(/id="(lexical|notation|grammar)/);
  });

  it('ignores "Example N.M" subheadings', () => {
    const out = process('<h3>Example 2.3.1</h3>');
    expect(out).not.toMatch(/id="(lexical|notation|grammar)/);
  });

  it('leaves unrelated headings alone', () => {
    const out = process('<h2>Installation</h2>');
    expect(out).not.toContain('id=');
  });
});
```

- [ ] **Step 2: Run the test suite to confirm failures**

Run: `cd standard && pnpm test -- rehype-clause-anchors`
Expected: All slug-reading tests fail (plugin still emits `sec-N-M-K`).

- [ ] **Step 3: Rewrite the plugin**

```typescript
import type { Root, Element, Text } from 'hast';
import { visit } from 'unist-util-visit';

// Clause-number prefix at start of heading text. N is a digit run or a
// single uppercase letter; optional .M[.K] subsection numbers.
const CLAUSE_RE = /^([0-9]+|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\.(\s|$)/;

// Trailing [#slug] marker. Slug grammar: chapter.leaf with dash-separated
// words; single level (bare chapter) also accepted.
const MARKER_RE = /\s+\[#([^\]]+)\]\s*$/;
const SLUG_RE = /^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)?$/;

function extractText(el: Element): string {
  let out = '';
  for (const child of el.children) {
    if (child.type === 'text') out += child.value;
    else if (child.type === 'element') out += extractText(child as Element);
  }
  return out;
}

function stripMarkerFromLastText(el: Element): void {
  // Walk children in reverse; the marker lives in the trailing text node.
  for (let i = el.children.length - 1; i >= 0; i--) {
    const c = el.children[i];
    if (c.type === 'text') {
      const t = c as Text;
      const m = t.value.match(MARKER_RE);
      if (m) {
        t.value = t.value.slice(0, m.index);
      }
      return;
    }
    if (c.type === 'element') {
      stripMarkerFromLastText(c as Element);
      return;
    }
  }
}

export function rehypeClauseAnchors() {
  return function transformer(tree: Root) {
    const seen = new Map<string, string>();
    visit(tree, 'element', (node: Element) => {
      if (!/^h[1-6]$/.test(node.tagName)) return;
      const text = extractText(node);
      if (!CLAUSE_RE.test(text)) return;

      const markerMatch = text.match(MARKER_RE);
      if (!markerMatch) {
        throw new Error(
          `clause-numbered heading missing [#slug] marker: "${text}"`,
        );
      }
      const slug = markerMatch[1];
      if (!SLUG_RE.test(slug)) {
        throw new Error(
          `invalid slug "${slug}" in heading "${text}" — must match [a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)?`,
        );
      }
      if (seen.has(slug)) {
        throw new Error(
          `duplicate slug "${slug}": appears in both "${seen.get(slug)}" and "${text}"`,
        );
      }
      seen.set(slug, text);

      stripMarkerFromLastText(node);
      node.properties = node.properties ?? {};
      (node.properties as Record<string, unknown>).id = slug;
    });
  };
}

export default rehypeClauseAnchors;
```

- [ ] **Step 4: Run the test suite to confirm all pass**

Run: `cd standard && pnpm test -- rehype-clause-anchors`
Expected: All 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add standard/src/plugins/rehype-clause-anchors.ts \
        standard/src/plugins/__tests__/rehype-clause-anchors.test.ts
git commit -m "feat(standard): rehype-clause-anchors reads [#slug] markers

Replaces positional sec-N-M-K anchor synthesis with explicit slug
markers of the form [#chapter.leaf] at end of numbered headings.
Fails the build on missing marker, invalid slug grammar, or duplicate
slug across the document set."
```

---

## Task 3: Rewrite `rehype-clause-xrefs.ts`

**Files:**
- Modify: `standard/src/plugins/rehype-clause-xrefs.ts`
- Test: `standard/src/plugins/__tests__/rehype-clause-xrefs.test.ts`

- [ ] **Step 1: Replace the existing test file**

```typescript
import { describe, it, expect } from 'vitest';
import { rehype } from 'rehype';
import { rehypeClauseXrefs } from '../rehype-clause-xrefs';
import type { ClauseInfo } from '../clauses';

function makeMap(entries: Array<[string, ClauseInfo]>): Map<string, ClauseInfo> {
  return new Map(entries);
}

function process(
  html: string,
  clauseMap: Map<string, ClauseInfo>,
): string {
  return String(
    rehype()
      .data('settings', { fragment: true })
      .use(rehypeClauseXrefs, { clauseMap })
      .processSync(html),
  );
}

describe('rehypeClauseXrefs', () => {
  const defaultMap = makeMap([
    ['lexical.identifiers', {
      href: '/02-lexical/#lexical.identifiers',
      number: '2.3',
      text: 'Identifiers',
    }],
    ['recipes.multi-output-cook', {
      href: '/04-recipes/#recipes.multi-output-cook',
      number: '4.6.2',
      text: 'Multi-output cook',
    }],
    ['grammar-appendix.top-level', {
      href: '/appendix/a-grammar/#grammar-appendix.top-level',
      number: 'A.3',
      text: 'Top level',
    }],
    ['xref', {
      href: '/05-cross-recipe-references/#xref',
      number: '5',
      text: 'Cross-recipe references',
    }],
  ]);

  it('links a two-level §{slug} reference', () => {
    const out = process('<p>See §{lexical.identifiers} for details.</p>', defaultMap);
    expect(out).toContain('href="/02-lexical/#lexical.identifiers"');
    expect(out).toContain('class="clause-xref"');
    expect(out).toContain('title="2.3. Identifiers"');
    expect(out).toContain('>§ 2.3</a>');
  });

  it('links a three-level §{slug} reference', () => {
    const out = process('<p>See §{recipes.multi-output-cook}.</p>', defaultMap);
    expect(out).toContain('href="/04-recipes/#recipes.multi-output-cook"');
    expect(out).toContain('>§ 4.6.2</a>');
  });

  it('links an appendix §{slug} reference', () => {
    const out = process('<p>See §{grammar-appendix.top-level}.</p>', defaultMap);
    expect(out).toContain('href="/appendix/a-grammar/#grammar-appendix.top-level"');
    expect(out).toContain('>§ A.3</a>');
  });

  it('links a chapter-level §{slug} reference', () => {
    const out = process('<p>See §{xref}.</p>', defaultMap);
    expect(out).toContain('href="/05-cross-recipe-references/#xref"');
    expect(out).toContain('>§ 5</a>');
  });

  it('throws on an unresolved slug', () => {
    expect(() =>
      process('<p>See §{nope.missing}.</p>', defaultMap),
    ).toThrowError(/unknown clause slug "nope\.missing"/i);
  });

  it('handles multiple refs in one paragraph', () => {
    const out = process(
      '<p>Compare §{lexical.identifiers} with §{grammar-appendix.top-level}.</p>',
      defaultMap,
    );
    expect(out.match(/class="clause-xref"/g) || []).toHaveLength(2);
  });

  it('does not wrap §{slug} inside inline code', () => {
    const out = process(
      '<p>The token <code>§{lexical.identifiers}</code> is literal.</p>',
      defaultMap,
    );
    expect(out).toContain('<code>§{lexical.identifiers}</code>');
    expect(out).not.toContain('class="clause-xref"');
  });

  it('does not wrap §{slug} that is already inside a link', () => {
    const out = process(
      '<p>See <a href="/elsewhere">§{lexical.identifiers}</a> too.</p>',
      defaultMap,
    );
    expect(out).toContain('<a href="/elsewhere">§{lexical.identifiers}</a>');
    expect(out).not.toContain('href="/02-lexical/#lexical.identifiers"');
  });

  it('links a ref that follows a closed <a> in the same paragraph', () => {
    const out = process(
      '<p>See <a href="/x">link</a> and §{lexical.identifiers} for details.</p>',
      defaultMap,
    );
    expect(out).toContain('<a href="/x">link</a>');
    expect(out).toContain('href="/02-lexical/#lexical.identifiers"');
    expect(out).toContain('>§ 2.3</a>');
  });
});
```

- [ ] **Step 2: Run the test suite to confirm failures**

Run: `cd standard && pnpm test -- rehype-clause-xrefs`
Expected: All tests fail (plugin still uses numeric regex + silent fallback).

- [ ] **Step 3: Rewrite the plugin**

```typescript
import type { Root, Element, Text, Parent } from 'hast';
import type { ClauseInfo } from './clauses.ts';

export interface ClauseXrefsOptions {
  clauseMap: Map<string, ClauseInfo>;
}

// Matches `§{chapter.leaf}` in prose. The braces delimit the slug so no
// word-boundary heuristics are needed. Slug characters: lowercase letters,
// digits, dash, and a single dot separating chapter from leaf.
const XREF_RE = /§\{([a-z0-9.-]+)\}/g;

const SKIP_TAGS = new Set(['a', 'code', 'pre']);

export function rehypeClauseXrefs(options: ClauseXrefsOptions) {
  const { clauseMap } = options;

  return function transformer(tree: Root) {
    const targets: Array<{ parent: Parent; index: number; value: string }> = [];
    collectTextNodes(tree, [], targets);

    for (let t = targets.length - 1; t >= 0; t--) {
      const { parent, index, value } = targets[t];
      const matches = [...value.matchAll(XREF_RE)];
      if (matches.length === 0) continue;

      const newChildren: Array<Text | Element> = [];
      let last = 0;
      for (const m of matches) {
        const full = m[0];
        const slug = m[1];
        const info = clauseMap.get(slug);
        if (!info) {
          throw new Error(
            `unknown clause slug "${slug}" — §{${slug}} did not resolve in clauseMap`,
          );
        }

        if (m.index! > last) {
          newChildren.push({ type: 'text', value: value.slice(last, m.index) });
        }
        newChildren.push({
          type: 'element',
          tagName: 'a',
          properties: {
            href: info.href,
            className: ['clause-xref'],
            title: `${info.number}. ${info.text}`,
          },
          children: [{ type: 'text', value: `§ ${info.number}` }],
        });
        last = m.index! + full.length;
      }
      if (last < value.length) {
        newChildren.push({ type: 'text', value: value.slice(last) });
      }

      parent.children.splice(index, 1, ...newChildren);
    }
  };
}

function collectTextNodes(
  node: Root | Element,
  ancestors: Element[],
  out: Array<{ parent: Parent; index: number; value: string }>,
): void {
  const parent = node as unknown as Parent;
  for (let i = 0; i < parent.children.length; i++) {
    const child = parent.children[i];
    if (child.type === 'text') {
      const skip = ancestors.some(a => SKIP_TAGS.has(a.tagName));
      if (!skip) {
        out.push({ parent, index: i, value: (child as Text).value });
      }
    } else if (child.type === 'element') {
      ancestors.push(child as Element);
      collectTextNodes(child as Element, ancestors, out);
      ancestors.pop();
    }
  }
}

export default rehypeClauseXrefs;
```

- [ ] **Step 4: Run the test suite to confirm all pass**

Run: `cd standard && pnpm test -- rehype-clause-xrefs`
Expected: All 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add standard/src/plugins/rehype-clause-xrefs.ts \
        standard/src/plugins/__tests__/rehype-clause-xrefs.test.ts
git commit -m "feat(standard): rehype-clause-xrefs resolves §{slug} refs

Replaces positional § N.M parsing with explicit §{chapter.leaf} refs
resolved through the slug-keyed clause registry. Unknown slugs raise
a build error naming the slug (no silent plain-text fallback). The
rendered anchor text is pulled live from the heading's current number,
so renumbering propagates on the next build without a prose diff."
```

---

## Task 4: New `rehype-bare-ref-lint.ts`

**Files:**
- Create: `standard/src/plugins/rehype-bare-ref-lint.ts`
- Create: `standard/src/plugins/__tests__/rehype-bare-ref-lint.test.ts`

- [ ] **Step 1: Write the failing test**

```typescript
import { describe, it, expect } from 'vitest';
import { rehype } from 'rehype';
import { rehypeBareRefLint } from '../rehype-bare-ref-lint';

function process(html: string): string {
  return String(
    rehype()
      .data('settings', { fragment: true })
      .use(rehypeBareRefLint)
      .processSync(html),
  );
}

describe('rehypeBareRefLint', () => {
  it('passes clean prose with no § refs', () => {
    expect(() => process('<p>Nothing to see here.</p>')).not.toThrow();
  });

  it('passes prose with §{slug} refs', () => {
    expect(() =>
      process('<p>See §{lexical.identifiers}.</p>'),
    ).not.toThrow();
  });

  it('throws on bare § 2.3', () => {
    expect(() => process('<p>See § 2.3.</p>')).toThrowError(
      /bare numeric § reference "§ 2\.3"/i,
    );
  });

  it('throws on bare § N.M.K', () => {
    expect(() => process('<p>See § 4.6.2 for more.</p>')).toThrowError(
      /bare numeric § reference "§ 4\.6\.2"/i,
    );
  });

  it('throws on bare § A.3 (appendix)', () => {
    expect(() => process('<p>See § A.3.</p>')).toThrowError(
      /bare numeric § reference "§ A\.3"/i,
    );
  });

  it('throws on bare § 5 (chapter)', () => {
    expect(() => process('<p>See § 5.</p>')).toThrowError(
      /bare numeric § reference "§ 5"/i,
    );
  });

  it('ignores § inside inline code', () => {
    expect(() =>
      process('<p>The token <code>§ 2.3</code> is literal.</p>'),
    ).not.toThrow();
  });

  it('ignores § inside <pre> blocks', () => {
    expect(() =>
      process('<pre><code>§ 2.3</code></pre>'),
    ).not.toThrow();
  });

  it('ignores § inside existing <a> (already-linked legacy)', () => {
    expect(() =>
      process('<p><a href="/x">§ 2.3</a></p>'),
    ).not.toThrow();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd standard && pnpm test -- rehype-bare-ref-lint`
Expected: FAIL with "Cannot find module '../rehype-bare-ref-lint'".

- [ ] **Step 3: Write the plugin**

```typescript
import type { Root, Element, Text, Parent } from 'hast';

// Matches bare `§ N[.M[.K]]` or `§ X[.N[.M]]` (X = single uppercase letter).
// Leading `§ ` includes a literal space; the ref ends at a word boundary so
// we don't match mid-identifier.
const BARE_REF_RE = /§\s+([0-9]+|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\b/g;

const SKIP_TAGS = new Set(['a', 'code', 'pre']);

export function rehypeBareRefLint() {
  return function transformer(tree: Root) {
    const hits: string[] = [];
    walk(tree, [], hits);
    if (hits.length > 0) {
      const list = hits.map(h => `  - "${h}"`).join('\n');
      throw new Error(
        `bare numeric § reference(s) found in prose — use §{chapter.leaf} slug form instead:\n${list}`,
      );
    }
  };
}

function walk(node: Root | Element, ancestors: Element[], out: string[]): void {
  const parent = node as unknown as Parent;
  for (const child of parent.children) {
    if (child.type === 'text') {
      const skip = ancestors.some(a => SKIP_TAGS.has(a.tagName));
      if (skip) continue;
      for (const m of (child as Text).value.matchAll(BARE_REF_RE)) {
        out.push(m[0]);
      }
    } else if (child.type === 'element') {
      ancestors.push(child as Element);
      walk(child as Element, ancestors, out);
      ancestors.pop();
    }
  }
}

export default rehypeBareRefLint;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd standard && pnpm test -- rehype-bare-ref-lint`
Expected: All 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add standard/src/plugins/rehype-bare-ref-lint.ts \
        standard/src/plugins/__tests__/rehype-bare-ref-lint.test.ts
git commit -m "feat(standard): add rehype-bare-ref-lint

Fails the build on bare numeric § refs (§ 2.3, § A.4, § 5) in prose.
All cross-references MUST use the §{chapter.leaf} slug form; the lint
prevents regressions after the migration lands. Refs inside <code>,
<pre>, and <a> are ignored."
```

---

## Task 5: Author the slug mapping table

**Files:**
- Create: `standard/scripts/slug-mapping.ts`

- [ ] **Step 1: Write the mapping table**

This is the single source of truth for every clause's slug. Each entry maps the current positional anchor ID (`sec-N-M-K` per the pre-migration `rehype-clause-anchors.ts`) to the new slug.

Use the following slugs. Leaves are kebab-case paraphrases of heading titles; deepest-level subsections use descriptive leaves rather than nested namespaces (per design § 3.1).

```typescript
// Slug mapping for the Cook Standard.
//
// Keyed by the pre-migration positional anchor ID (sec-N-M-K). The migration
// script in this directory reads this map to rewrite headings and refs.
// After migration, this file remains in-tree as the authoritative registry
// of chapter prefixes and their clauses — future renames update it alongside
// the heading markers.

export const SLUG_MAPPING: Record<string, string> = {
  // Chapter 0 — Introduction
  'sec-0':       'intro',
  'sec-0-1':     'intro.purpose',
  'sec-0-2':     'intro.scope',
  'sec-0-3':     'intro.non-scope',
  'sec-0-4':     'intro.normative-and-informative',
  'sec-0-5':     'intro.version-stance',
  'sec-0-6':     'intro.relationship-to-architecture',
  'sec-0-7':     'intro.conformance',

  // Chapter 1 — Notation and conventions
  'sec-1':       'notation',
  'sec-1-1':     'notation.keywords',
  'sec-1-2':     'notation.numbering-and-citation',
  'sec-1-3':     'notation.normative-informative-blocks',
  'sec-1-4':     'notation.grammar',
  'sec-1-5':     'notation.grammar-disagreement-precedence',
  'sec-1-6':     'notation.amendment-markers',
  'sec-1-7':     'notation.stable-anchors',

  // Chapter 2 — Lexical structure
  'sec-2':       'lexical',
  'sec-2-1':     'lexical.source-representation',
  'sec-2-2':     'lexical.tokens',
  'sec-2-3':     'lexical.identifiers',
  'sec-2-4':     'lexical.keywords',
  'sec-2-5':     'lexical.strings',
  'sec-2-6':     'lexical.comments',
  'sec-2-7':     'lexical.newlines-and-blank-lines',
  // (…continue for every current sec-N-M-K heading id in the document set.
  // The migration script enumerates all of them and aborts if any is
  // missing from this map.)
};
```

Enumerate every clause. The exhaustive list is derived from the current content — run this helper once to seed the file before review:

```bash
cd standard
grep -rhoE '^#{1,6}\s+[0-9A-Z][0-9A-Z.]*\.\s+.*$' src/content/docs/ \
  | sort -u
```

Use each heading's title to propose a kebab-case leaf. Keep leaves short; prefer concept words over structural words (e.g., `dep-list`, not `list-of-deps`). Review the full map before moving to Task 6 — this is the one place where slug quality is evaluated, and every subsequent rewrite depends on it.

- [ ] **Step 2: Run the duplicate check**

```bash
cd standard
node --input-type=module -e '
import { SLUG_MAPPING } from "./scripts/slug-mapping.ts";
const seen = new Map();
for (const [id, slug] of Object.entries(SLUG_MAPPING)) {
  if (seen.has(slug)) {
    console.error(`duplicate: ${slug} (${seen.get(slug)} and ${id})`);
    process.exit(1);
  }
  seen.set(slug, id);
}
console.log("ok: " + Object.keys(SLUG_MAPPING).length + " slugs, all unique");
'
```

Expected: `ok: <N> slugs, all unique`.

- [ ] **Step 3: Commit**

```bash
git add standard/scripts/slug-mapping.ts
git commit -m "feat(standard): author slug mapping table

Maps every current sec-N-M-K heading id to its new chapter.leaf slug.
The migration script in the next commit reads this table to rewrite
headings and refs across the content set. The table remains in-tree
as the authoritative registry of chapter prefixes."
```

---

## Task 6: Write the migration script

**Files:**
- Create: `standard/scripts/migrate-slugs.mjs`

- [ ] **Step 1: Write the script**

```javascript
#!/usr/bin/env node
// One-shot migration: append [#<slug>] to every numbered heading and
// rewrite every `§ N.M[.K]` ref to `§{<slug>}`. Idempotent when rerun
// over already-migrated content (the second pass is a no-op because
// headings already have markers and refs already use §{} syntax).

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONTENT_ROOT = path.join(__dirname, '..', 'src', 'content', 'docs');
const MAPPING_PATH = path.join(__dirname, 'slug-mapping.ts');

// Extract the SLUG_MAPPING object literal from the .ts file.
// Cheap hand-rolled parse — the table is a flat string→string record.
function loadMapping() {
  const src = fs.readFileSync(MAPPING_PATH, 'utf8');
  const start = src.indexOf('{');
  const end = src.lastIndexOf('}');
  const body = src.slice(start + 1, end);
  const out = {};
  for (const line of body.split('\n')) {
    const m = line.match(/^\s*['"]([^'"]+)['"]\s*:\s*['"]([^'"]+)['"]\s*,?\s*(?:\/\/.*)?$/);
    if (m) out[m[1]] = m[2];
  }
  return out;
}

function numberFromSecId(id) {
  // "sec-2-3" → "2.3"; "sec-A-3" → "A.3"; "sec-5" → "5"
  return id.replace(/^sec-/, '').replace(/-/g, '.');
}

function secIdFromNumber(num) {
  return 'sec-' + num.replace(/\./g, '-');
}

function walk(root) {
  const out = [];
  for (const e of fs.readdirSync(root, { withFileTypes: true })) {
    const p = path.join(root, e.name);
    if (e.isDirectory()) out.push(...walk(p));
    else if (e.isFile() && e.name.endsWith('.mdx')) out.push(p);
  }
  return out;
}

function rewriteHeadings(src, mapping, file, unmapped) {
  // Match numbered headings; append [#slug] if absent.
  return src.replace(
    /^(#+)\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\.(\s+)(.*?)(\s*)$/gm,
    (match, hashes, top, mid, bot, sep, title) => {
      // Already migrated? Skip.
      if (/\[#[a-z][a-z0-9.-]*\]\s*$/.test(title)) return match;
      const num = [top, mid, bot].filter(Boolean).join('.');
      const secId = secIdFromNumber(num);
      const slug = mapping[secId];
      if (!slug) {
        unmapped.push(`${file}: ${secId} "${title.trim()}"`);
        return match;
      }
      return `${hashes} ${num}. ${title.replace(/\s+$/, '')} [#${slug}]`;
    },
  );
}

function rewriteRefs(src, mapping, file, unmapped) {
  // Match bare § N.M refs. Skip inside fenced code blocks and inline code.
  // Simple approach: split on fence boundaries, rewrite only outside.
  const segments = src.split(/(```[\s\S]*?```|`[^`\n]*`)/g);
  for (let i = 0; i < segments.length; i++) {
    // Odd indices are code; even indices are prose.
    if (i % 2 === 1) continue;
    segments[i] = segments[i].replace(
      /§\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?(?=\b|[^0-9A-Za-z])/g,
      (match, top, mid, bot) => {
        const num = [top, mid, bot].filter(Boolean).join('.');
        const secId = secIdFromNumber(num);
        const slug = mapping[secId];
        if (!slug) {
          unmapped.push(`${file}: § ${num} (no mapping for ${secId})`);
          return match;
        }
        return `§{${slug}}`;
      },
    );
  }
  return segments.join('');
}

function main() {
  const mapping = loadMapping();
  const files = walk(CONTENT_ROOT);
  const unmapped = [];

  for (const abs of files) {
    const rel = path.relative(CONTENT_ROOT, abs);
    let src = fs.readFileSync(abs, 'utf8');
    src = rewriteHeadings(src, mapping, rel, unmapped);
    src = rewriteRefs(src, mapping, rel, unmapped);
    fs.writeFileSync(abs, src);
  }

  if (unmapped.length > 0) {
    console.error('Unmapped clauses encountered:');
    for (const u of unmapped) console.error('  ' + u);
    process.exit(1);
  }
  console.log(`Migrated ${files.length} files`);
}

main();
```

- [ ] **Step 2: Commit (no content changes yet)**

```bash
git add standard/scripts/migrate-slugs.mjs
git commit -m "feat(standard): migration script for slug-based refs

One-shot rewriter that reads slug-mapping.ts, appends [#slug] markers
to numbered headings, and rewrites § N.M refs to §{slug} form.
Idempotent when rerun. Exits non-zero if any clause in the content
set lacks a mapping entry."
```

---

## Task 7: Run the migration

**Files:**
- Modify: every `.mdx` under `standard/src/content/docs/`

- [ ] **Step 1: Dry-run by diff**

```bash
cd standard
node scripts/migrate-slugs.mjs
git diff --stat src/content/docs/
```

Expected: every `.mdx` in the content set appears in the diff with a mix of insertions (markers) and changes (refs).

- [ ] **Step 2: Spot-check the diff**

Open three files and confirm:
- `src/content/docs/02-lexical.mdx` — every `## N.M. …` now ends with `[#lexical.<leaf>]`; every `§ 2.X` reference is now `§{lexical.<leaf>}`.
- `src/content/docs/05-cross-recipe-references.mdx` — same, with `xref.*` prefixes.
- `src/content/docs/appendix/A-grammar.mdx` — `grammar-appendix.*` prefixes.

If a section looks wrong, fix the slug in `scripts/slug-mapping.ts`, then run:
```bash
git checkout -- src/content/docs/
node scripts/migrate-slugs.mjs
```

- [ ] **Step 3: Run the plugin tests one more time**

Run: `cd standard && pnpm test`
Expected: all tests pass (no plugin behavior changed; tests still green).

- [ ] **Step 4: Run the build**

Run: `cd standard && pnpm build`
Expected: the build either (a) succeeds, or (b) fails on the spec text in § 1.2 / § 1.7 that still references the old system — address in Task 8. Any other failure is a migration bug; resolve before moving on.

- [ ] **Step 5: Commit**

```bash
git add standard/src/content/docs/
git commit -m "migrate(standard): rewrite headings and refs to slug form

Applies scripts/migrate-slugs.mjs across the content set. Every
numbered heading now carries an explicit [#chapter.leaf] marker; every
§ N.M prose ref has been rewritten to §{chapter.leaf}. Produced by a
single mechanical pass; slug choices in scripts/slug-mapping.ts."
```

---

## Task 8: Update § 1.2 and § 1.7 spec text

**Files:**
- Modify: `standard/src/content/docs/01-notation.mdx`

- [ ] **Step 1: Rewrite § 1.2**

Replace the current § 1.2 block (lines 16–22 of the pre-migration file; post-migration the exact line numbers shift) with:

```mdx
## 1.2. Section numbering and citation. [#notation.numbering-and-citation]

Chapters are cited as `§ N`. Sections within a chapter are cited as `§ N.M`, subsections as `§ N.M.K`, and deepest-level subsections as `§ N.M.K.P`. Nesting deeper than four levels MUST NOT appear in this Standard.

Appendices share the `§` sigil: `§ A.1`, `§ A.2.3`. The rendered form of every citation is the live section number; the source form is a stable slug (see §{notation.stable-anchors}).

Examples within a section are numbered locally: `Example N.M.1`, `Example N.M.2`, and so on. Numbering restarts at 1 within each section.

Authors write cross-references in source as `§{chapter.leaf}`. The build replaces each with the linked numeric at render time. Bare numeric forms (`§ 2.3`) in prose are rejected by the build.
```

- [ ] **Step 2: Rewrite § 1.7**

Replace the current § 1.7 block with:

```mdx
## 1.7. Stable anchors. [#notation.stable-anchors]

Every numbered chunk in this Standard carries a stable slug so that cross-references survive renumbering and retitling. Slugs match the grammar `chapter.leaf`, where each segment is `[a-z][a-z0-9-]*` and the chapter prefix is drawn from the following fixed set:

| Chapter                                 | Prefix               |
| --------------------------------------- | -------------------- |
| 0. Introduction                         | `intro`              |
| 1. Notation and conventions             | `notation`           |
| 2. Lexical structure                    | `lexical`            |
| 3. Syntactic grammar                    | `grammar`            |
| 4. Recipes and step kinds               | `recipes`            |
| 5. Cross-recipe references              | `xref`               |
| 6. Cook Lua API                         | `lua`                |
| 7. Cross-Cookfile composition           | `modules`            |
| 8. Execution model                      | `exec`               |
| A. Grammar (normative appendix)         | `grammar-appendix`   |
| B. Rationale (informative)              | `rationale`          |
| C. Examples (informative)               | `examples`           |
| D. Changes                              | `changes`            |

Authors declare the slug on a numbered heading with a trailing `[#<slug>]` marker:

```mdx
## 2.3. Identifiers. [#lexical.identifiers]
```

The build extracts the marker, applies it as the heading's HTML id, and strips it from the rendered text. Unresolved refs, duplicate slugs, missing markers, and invalid slug grammar all fail the build.

Authors MUST NOT change a slug without recording the change in `D-changes.mdx`. Renumbering a section does NOT require a D-changes entry — the slug is the stable identity, not the number.
```

- [ ] **Step 3: Run the build**

Run: `cd standard && pnpm build`
Expected: build succeeds. Any failure points to a slug-reference typo in the new § 1.7 content — fix and re-run.

- [ ] **Step 4: Commit**

```bash
git add standard/src/content/docs/01-notation.mdx
git commit -m "spec(standard): rewrite § 1.2 and § 1.7 for slug-based refs

Describes the §{chapter.leaf} source form, the chapter-prefix
registry, and the [#slug] heading marker. Makes § 1.7's \"stable
anchors\" claim correspond to an actual mechanism rather than the
pre-migration positional sec-N-M-K scheme."
```

---

## Task 9: Wire `rehype-bare-ref-lint` into astro.config.mjs

**Files:**
- Modify: `standard/astro.config.mjs`

- [ ] **Step 1: Add the import and plugin entry**

At the top of `astro.config.mjs`, add the import next to the other plugin imports:

```javascript
import { rehypeBareRefLint } from './src/plugins/rehype-bare-ref-lint.ts';
```

In the `rehypePlugins` array, add `rehypeBareRefLint` as the **last** entry so it runs after `rehypeClauseXrefs` has already wrapped every valid `§{slug}` — at which point any remaining bare `§ N.M` in a text node is genuinely a bug:

```javascript
rehypePlugins: [
  rehypeClauseAnchors,
  [rehypeClauseXrefs, { clauseMap }],
  [rehypeCsPermalinks, {
    knownIds: knownCsIds,
    changesHref: '/appendix/d-changes/',
  }],
  rehypeBareRefLint,
],
```

- [ ] **Step 2: Run the build**

Run: `cd standard && pnpm build`
Expected: build succeeds; the lint finds zero bare refs post-migration.

- [ ] **Step 3: Sanity-check the lint by poisoning the content**

```bash
cd standard
# Insert a bare ref temporarily
sed -i '0,/^## /s/^## /See § 2.3 here.\n\n## /' src/content/docs/02-lexical.mdx
pnpm build
# Expected: build fails with "bare numeric § reference(s) found"
git checkout -- src/content/docs/02-lexical.mdx
```

Expected: build fails loudly with the lint error; `git checkout` restores clean state.

- [ ] **Step 4: Commit**

```bash
git add standard/astro.config.mjs
git commit -m "feat(standard): wire rehype-bare-ref-lint into the build

Runs after rehype-clause-xrefs so every remaining bare § N.M in a
prose text node is a true regression, not an unexpanded macro. Catches
accidental reintroduction of positional citation forms in future PRs."
```

---

## Task 10: Add D-changes entry

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`

- [ ] **Step 1: Read the existing D-changes format**

Open `standard/src/content/docs/appendix/D-changes.mdx`. Identify the highest existing CS-NNNN number (e.g. `CS-0009`) and the entry format used in the file. The new CS number is one greater.

- [ ] **Step 2: Append the new entry**

Append an entry following the established format. Minimum content:

```mdx
### CS-NNNN — Slug-based cross-references [#changes.cs-NNNN]

Cross-references in this Standard are now author-assigned stable slugs of the form `chapter.leaf`. Prose refs use `§{chapter.leaf}` in source and render as the current section number. Heading anchors use the slug directly (URLs such as `/02-lexical/#lexical.identifiers`). The prior positional `sec-N-M-K` anchor scheme is removed.

**Motivation.** Section renumbering previously drifted every ref pointing at the affected sections. The fix commits after the CS-0009 restructure (`35d46d9`, `e83c490`) repaired the visible drift; the underlying positional anchor scheme was the root cause. Slug-based refs decouple identity from ordering.

**Impact.** Prose sources changed in bulk; rendered output unchanged. External deep links using the old `#sec-N-M-K` anchors break — the Standard is pre-release, no external consumers were affected.

**Conformance.** No impact. The change is authoring-surface only.
```

Replace `NNNN` with the allocated number throughout.

- [ ] **Step 3: Run the build**

Run: `cd standard && pnpm build`
Expected: build succeeds; the new CS ID is registered and links resolve.

- [ ] **Step 4: Commit**

```bash
git add standard/src/content/docs/appendix/D-changes.mdx
git commit -m "spec(standard): add CS-NNNN D-changes entry for slug refs

Records the migration from positional sec-N-M-K anchors to author-
assigned chapter.leaf slugs. Motivation, impact, and conformance
assessment included per the D-changes template."
```

(Adjust CS number in the commit message to match the allocated ID.)

---

## Task 11: Final verification

- [ ] **Step 1: Fresh build from clean state**

```bash
cd standard
rm -rf dist
pnpm build
```

Expected: successful build with no warnings about missing or unresolved slugs.

- [ ] **Step 2: Run the full test suite**

Run: `cd standard && pnpm test`
Expected: all suites pass (clause-anchors, clause-xrefs, bare-ref-lint, cs-permalinks, cook-highlight, rfc2119).

- [ ] **Step 3: Smoke-check a rendered page**

```bash
cd standard && pnpm preview &
curl -s http://localhost:4321/02-lexical/ | grep -o 'id="lexical\.[a-z-]*"' | sort -u | head -5
curl -s http://localhost:4321/02-lexical/ | grep -o 'href="/02-lexical/#lexical\.[a-z-]*"' | sort -u | head -5
# Kill the preview server
kill %1
```

Expected: both grep outputs show slug-form IDs and hrefs; no `sec-N-M-K` fragments.

- [ ] **Step 4: Confirm rendered link text still says `§ N.M`**

```bash
curl -s http://localhost:4321/02-lexical/ | grep -oE '>§ [0-9A-Z][0-9.]*<' | sort -u | head -5
```

Expected: entries like `>§ 2.3<`, `>§ 4.6<` — link text is the numeric, as designed.

- [ ] **Step 5: Run the keyword lint (unrelated, but catches spec regressions)**

Run: `cd standard && pnpm lint:keywords`
Expected: passes; no RFC 2119 keyword regressions introduced during § 1.2 / § 1.7 rewrite.

- [ ] **Step 6: Final rebase/squash check and summary**

```bash
git log --oneline origin/main..HEAD
```

Expected: the commits from Tasks 1–10 in order, forming a self-contained branch ready for review.

---

## Self-Review Notes

**Spec coverage:** Every numbered section of the design doc maps to a task:
- Design § 3.1 (slug grammar) → Tasks 2, 5 (enforced by plugin + mapping)
- Design § 3.2 (heading declaration) → Task 2, Task 7
- Design § 3.3 (citation syntax) → Task 3, Task 7
- Design § 3.4 (HTML anchors) → Task 2
- Design § 3.5 (validation) → Tasks 1, 2, 3, 4, 9
- Design § 3.6 (spec text updates) → Tasks 8, 10
- Design § 4.1 (plugin changes) → Tasks 1–4, 9
- Design § 4.2 (migration script) → Tasks 5, 6, 7
- Design § 4.3 (order of operations) → Task sequence 1 → 11

**Type consistency:** `ClauseInfo` gains `number` and `text` fields, loses positional-only semantics; all three plugins/tests use the new shape. Slug regex appears in three files — the grammar `[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)?` is identical in `clauses.ts`, `rehype-clause-anchors.ts`, and the mapping-table duplicate check.

**Placeholders:** `CS-NNNN` is the one intentional placeholder, resolved at Task 10 step 2 when the author reads the file. Every code block is complete.
