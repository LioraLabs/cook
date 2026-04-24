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
