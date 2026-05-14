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

// Historical-content files legitimately reference retired slugs by name —
// the Rationale appendix discusses prior content under its old slug, and
// the Changes appendix records the rename in CS entries. These are exempt
// from the source-lint; the rename registry will still route any rendered
// links via `[slug-renames]` (informational, not fatal).
const HISTORICAL_FILES = new Set([
  'appendix/C-rationale.mdx',
  'appendix/E-changes.mdx',
]);

function relFromContentRoot(file: string): string {
  return path.relative(CONTENT_ROOT, file).split(path.sep).join('/');
}

describe('source files contain no retired slugs', () => {
  it('no §{retired-slug} in any rendered source', () => {
    const offences: string[] = [];
    for (const file of walkMdx(CONTENT_ROOT)) {
      const rel = relFromContentRoot(file);
      if (HISTORICAL_FILES.has(rel)) continue;
      const text = fs.readFileSync(file, 'utf8');
      for (const match of text.matchAll(REF_RE)) {
        if (retired.includes(match[1])) {
          offences.push(`${rel}: §{${match[1]}}`);
        }
      }
      for (const match of text.matchAll(ANCHOR_RE)) {
        if (retired.includes(match[1])) {
          offences.push(`${rel}: [#${match[1]}]`);
        }
      }
    }
    expect(offences).toEqual([]);
  });
});
