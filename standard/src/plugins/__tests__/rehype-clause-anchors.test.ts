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
  it('emits deterministic id for a normative clause heading', () => {
    const out = process('<h2>2.3. Tokens</h2>');
    expect(out).toContain('id="sec-2-3"');
  });

  it('handles three-level clause numbers', () => {
    const out = process('<h3>4.6.2. Multi-output cook</h3>');
    expect(out).toContain('id="sec-4-6-2"');
  });

  it('handles appendix clause numbers', () => {
    const out = process('<h2>A.3. Top level</h2>');
    expect(out).toContain('id="sec-A-3"');
  });

  it('handles chapter-level headings (no sub-number)', () => {
    const out = process('<h1>2. Lexical structure</h1>');
    expect(out).toContain('id="sec-2"');
  });

  it('handles appendix-only headings', () => {
    const out = process('<h1>Appendix A. Grammar (normative)</h1>');
    // "Appendix" starts with 'A' which does satisfy [A-Z], but the regex
    // requires the next character to be '.' or another clause token. Here
    // the next character is 'p' ("ppendix"), so the match fails at the
    // required trailing period.
    expect(out).not.toContain('id="sec-');
  });

  it('ignores "Note N.M" subheadings (Note starts with letter, not number)', () => {
    const out = process('<h3>Note 2.1.1</h3>');
    expect(out).not.toContain('id="sec-');
  });

  it('ignores "Example N.M" subheadings', () => {
    const out = process('<h3>Example 2.3.1</h3>');
    expect(out).not.toContain('id="sec-');
  });

  it('leaves unrelated headings alone', () => {
    const out = process('<h2>Installation</h2>');
    expect(out).not.toContain('id="sec-');
  });

  it('throws on duplicate clause numbers', () => {
    const input = '<h2>2.3. First</h2><h2>2.3. Second</h2>';
    expect(() => process(input)).toThrowError(/duplicate clause number/i);
  });
});
