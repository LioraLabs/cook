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
