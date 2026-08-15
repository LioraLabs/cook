import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { SLUG_RENAMES } from '../../../scripts/slug-renames.ts';
import { SLUG_MAPPING } from '../../../scripts/slug-mapping.ts';

// COOK-486. `rehype-clause-xrefs` throws on a `§{slug}` that resolves in none
// of the three places it consults: the clause map harvested from headings, the
// rename registry, and the v0.10 mapping. That throw only fires during
// `astro build`, which cannot run without `prebuild`'s tree-sitter wasm — so a
// dangling reference lands silently and stays. This lint is that throw made
// runnable off the same three sources.

const CONTENT_ROOT = path.resolve(__dirname, '../../../src/content/docs');

// Mirrors `clauses.ts` HEADING_RE's marker: an anchor is minted by a heading
// and by nothing else, so a rule stated in body prose has no slug to link to.
const ANCHOR_RE = /^#+\s+.*\[#([a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)*)\]\s*$/gm;

// `rehype-clause-xrefs` XREF_RE.
const XREF_RE = /§\{([a-z0-9.-]+)\}/g;

function walkMdx(dir: string): string[] {
  const out: string[] = [];
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) out.push(...walkMdx(p));
    else if (ent.isFile() && p.endsWith('.mdx')) out.push(p);
  }
  return out;
}

// The renderer never substitutes inside `a` / `code` / `pre`, so a `§{…}` in a
// fence or in backticks cannot throw and is not a dangling reference. Blank
// those spans out rather than deleting them, to keep line numbers honest.
function blankCode(text: string): string {
  return text
    .replace(/^```[\s\S]*?^```/gm, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/`[^`\n]*`/g, (m) => ' '.repeat(m.length));
}

const files = walkMdx(CONTENT_ROOT);
const sources = new Map(files.map((f) => [f, fs.readFileSync(f, 'utf8')]));

const resolvable = new Set<string>([
  ...Object.keys(SLUG_RENAMES),
  ...Object.values(SLUG_MAPPING),
]);
for (const text of sources.values()) {
  for (const m of text.matchAll(ANCHOR_RE)) resolvable.add(m[1]);
}

describe('every clause xref resolves', () => {
  it('no §{slug} is dangling', () => {
    const offences: string[] = [];
    for (const [file, raw] of sources) {
      const rel = path.relative(CONTENT_ROOT, file).split(path.sep).join('/');
      const text = blankCode(raw);
      for (const m of text.matchAll(XREF_RE)) {
        if (resolvable.has(m[1])) continue;
        const line = text.slice(0, m.index).split('\n').length;
        offences.push(`${rel}:${line}: §{${m[1]}}`);
      }
    }
    expect(offences).toEqual([]);
  });
});
