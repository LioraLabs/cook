import { describe, it, expect } from 'vitest';
import { rehype } from 'rehype';
import { rehypeClauseXrefs } from '../rehype-clause-xrefs';
import type { ClauseInfo } from '../clauses';

function makeMap(entries: Array<[string, ClauseInfo]>): Map<string, ClauseInfo> {
  return new Map(entries);
}

function process(html: string, clauseMap: Map<string, ClauseInfo>): string {
  return String(
    rehype()
      .data('settings', { fragment: true })
      .use(rehypeClauseXrefs, { clauseMap })
      .processSync(html),
  );
}

describe('rehypeClauseXrefs', () => {
  const defaultMap = makeMap([
    ['sec-2-3', { href: '/02-lexical/#sec-2-3', text: '2.3. Identifiers' }],
    ['sec-4-6-2', { href: '/04-recipes/#sec-4-6-2', text: '4.6.2. Multi-output cook' }],
    ['sec-A-3', { href: '/appendix/a-grammar/#sec-A-3', text: 'A.3. Top level' }],
    ['sec-5', { href: '/05-cross-recipe-references/', text: '5. Cross-recipe references' }],
    ['sec-8', { href: '/08-execution-model/', text: '8. Execution model' }],
  ]);

  it('links a two-level § X.Y reference', () => {
    const out = process('<p>See § 2.3 for details.</p>', defaultMap);
    expect(out).toContain('href="/02-lexical/#sec-2-3"');
    expect(out).toContain('class="clause-xref"');
    expect(out).toContain('title="2.3. Identifiers"');
    expect(out).toContain('>§ 2.3</a>');
  });

  it('links a three-level § X.Y.Z reference', () => {
    const out = process('<p>See § 4.6.2.</p>', defaultMap);
    expect(out).toContain('href="/04-recipes/#sec-4-6-2"');
    expect(out).toContain('>§ 4.6.2</a>');
  });

  it('links an appendix reference', () => {
    const out = process('<p>See § A.3.</p>', defaultMap);
    expect(out).toContain('href="/appendix/a-grammar/#sec-A-3"');
    expect(out).toContain('>§ A.3</a>');
  });

  it('links a chapter-level § X reference', () => {
    const out = process('<p>See § 5.</p>', defaultMap);
    expect(out).toContain('href="/05-cross-recipe-references/"');
    expect(out).toContain('>§ 5</a>');
  });

  it('leaves unresolved refs as plain text without failing', () => {
    const out = process('<p>Where § N is a placeholder.</p>', defaultMap);
    expect(out).not.toContain('class="clause-xref"');
    expect(out).toContain('§ N');
  });

  it('handles multiple refs in one paragraph', () => {
    const out = process('<p>Compare § 2.3 with § A.3.</p>', defaultMap);
    expect(out.match(/class="clause-xref"/g) || []).toHaveLength(2);
  });

  it('does not wrap § X.Y inside inline code', () => {
    const out = process('<p>The token <code>§ 2.3</code> is literal.</p>', defaultMap);
    expect(out).toContain('<code>§ 2.3</code>');
    expect(out).not.toContain('class="clause-xref"');
  });

  it('does not wrap § X.Y that is already inside a link', () => {
    const out = process('<p>See <a href="/elsewhere">§ 2.3</a> too.</p>', defaultMap);
    expect(out).toContain('<a href="/elsewhere">§ 2.3</a>');
    expect(out).not.toContain('href="/02-lexical/#sec-2-3"');
  });

  it('links a ref that follows a closed <a> in the same paragraph (ancestor-tracking regression)', () => {
    const out = process(
      '<p>See <a href="/x">link</a> and § 2.3 for details.</p>',
      defaultMap,
    );
    expect(out).toContain('<a href="/x">link</a>');
    expect(out).toContain('href="/02-lexical/#sec-2-3"');
    expect(out).toContain('>§ 2.3</a>');
  });
});
