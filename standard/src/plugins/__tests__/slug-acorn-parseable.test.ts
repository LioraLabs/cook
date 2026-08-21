import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { remark } from 'remark';
import remarkMdx from 'remark-mdx';

// MDX hands `§{slug}` to acorn as a JS expression before remark-slug-xrefs
// can collapse it back to text, so a slug whose hyphen-separated segment is
// a reserved word in expression position (`-in`, `-var-`, `-export`, ...)
// breaks `astro build` — and nothing else catches it, because the plugin
// unit tests never parse the content. This test parses every slug in the
// corpus (both §{refs} and [#anchors], since every anchor is a future ref)
// through the same remark-mdx pipeline, so the next keyword-suffixed slug
// fails here in seconds instead of in a full build. See COOK-254, COOK-337.

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
const REF_RE = /§\{([a-z][a-z0-9.\-]*)\}/g;
const ANCHOR_RE = /\[#([a-z][a-z0-9.\-]*)\]/g;

// CS permalink anchors ([#cs-0067], [#changes.cs-0188]) are addressed by
// rehype-cs-permalinks, never by §{...} xrefs, and their numeric segments
// (`0067` is an illegal numeric literal) can never parse as expressions.
const CS_PERMALINK_RE = /^(changes\.)?cs-\d+$/;

const mdx = remark().use(remarkMdx);

function parseableAsMdxExpression(slug: string): boolean {
  try {
    mdx.parse(`x §{${slug}} y`);
    return true;
  } catch {
    return false;
  }
}

describe('every slug survives MDX expression parsing', () => {
  it('no §{ref} or [#anchor] slug chokes acorn', () => {
    const slugs = new Map<string, string>(); // slug -> first sighting
    for (const file of walkMdx(CONTENT_ROOT)) {
      const text = fs.readFileSync(file, 'utf8');
      const rel = path.relative(CONTENT_ROOT, file);
      for (const re of [REF_RE, ANCHOR_RE]) {
        for (const m of text.matchAll(re)) {
          if (CS_PERMALINK_RE.test(m[1])) continue;
          if (!slugs.has(m[1])) slugs.set(m[1], rel);
        }
      }
    }
    expect(slugs.size).toBeGreaterThan(0);
    const offences = [...slugs]
      .filter(([slug]) => !parseableAsMdxExpression(slug))
      .map(([slug, file]) => `${slug} (first seen in ${file})`);
    expect(offences, 'rename these slugs — a hyphen-separated segment is a JS reserved word').toEqual([]);
  });
});
