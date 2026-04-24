import { describe, it, expect } from 'vitest';
import { remark } from 'remark';
import remarkMdx from 'remark-mdx';
import { toHast } from 'mdast-util-to-hast';
import { visit } from 'unist-util-visit';
import type { Root } from 'mdast';
import type { Text } from 'hast';
import { remarkSlugXrefs } from '../remark-slug-xrefs';

// Process source through remark-mdx + our plugin, then convert to hast.
// Returns all plain-text content from the hast as a single string —
// this mirrors exactly what rehype-clause-xrefs sees at build time.
async function processToHastText(source: string): Promise<string> {
  const proc = remark().use(remarkMdx).use(remarkSlugXrefs);
  const tree = proc.parse(source) as Root;
  await proc.run(tree);
  const hast = toHast(tree);

  const texts: string[] = [];
  visit(hast, 'text', (node: Text) => {
    texts.push(node.value);
  });
  return texts.join('');
}

describe('remarkSlugXrefs', () => {
  it('converts a two-level §{slug} back to plain text', async () => {
    const out = await processToHastText('See §{lexical.identifiers} for more.\n');
    expect(out).toContain('§{lexical.identifiers}');
    // The expression node must be gone — no mdxTextExpression residue.
    expect(out).not.toMatch(/^\{lexical\.identifiers\}/);
  });

  it('converts a chapter-level §{slug}', async () => {
    const out = await processToHastText('See §{xref} for cross-refs.\n');
    expect(out).toContain('§{xref}');
  });

  it('handles multiple §{slug} refs in one paragraph', async () => {
    const out = await processToHastText('Compare §{lexical.identifiers} with §{xref.names}.\n');
    expect(out).toContain('§{lexical.identifiers}');
    expect(out).toContain('§{xref.names}');
  });

  it('leaves non-slug expressions alone', async () => {
    // No § preceding {someVar} — the plugin must not touch it.
    // After mdx processing, the expression node stays as-is in mdast.
    // In hast text nodes it won't appear, which is fine — we just confirm
    // our plugin doesn't corrupt surrounding text.
    const proc = remark().use(remarkMdx).use(remarkSlugXrefs);
    const source = 'Value: {someVar} — end.\n';
    const tree = proc.parse(source) as Root;
    await proc.run(tree);

    // The mdxTextExpression node for `someVar` must still be present
    // (the plugin must not have removed or altered it).
    let found = false;
    visit(tree, 'mdxTextExpression', (node: { type: string; value: string }) => {
      if (node.value === 'someVar') found = true;
    });
    expect(found).toBe(true);
  });

  it('leaves § not followed by an expression alone', async () => {
    const out = await processToHastText('The § sigil on its own is fine.\n');
    expect(out).toContain('§ sigil');
  });
});
