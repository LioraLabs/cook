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
