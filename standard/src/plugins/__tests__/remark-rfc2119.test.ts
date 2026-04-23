import { describe, it, expect } from 'vitest';
import { remark } from 'remark';
import { remarkRfc2119 } from '../remark-rfc2119';

function renderToHast(input: string): string {
  // Run the plugin over a markdown input, then serialize the resulting mdast
  // back through remark-stringify. We're asserting on the *mdast shape* the
  // plugin emits, not final HTML — that keeps the test framework-free.
  const file = remark().use(remarkRfc2119).processSync(input);
  return String(file);
}

describe('remarkRfc2119', () => {
  it('wraps MUST in an html span with class rfc2119 rfc2119-must', () => {
    const out = renderToHast('An implementation MUST accept UTF-8 input.');
    expect(out).toContain('<span class="rfc2119 rfc2119-must">MUST</span>');
  });

  it('wraps MUST NOT as a single span (not two)', () => {
    const out = renderToHast('An implementation MUST NOT require a BOM.');
    expect(out).toContain('<span class="rfc2119 rfc2119-must">MUST NOT</span>');
  });

  it('wraps SHOULD and MAY distinctly', () => {
    const out = renderToHast('A tool SHOULD warn; it MAY abort.');
    expect(out).toContain('<span class="rfc2119 rfc2119-should">SHOULD</span>');
    expect(out).toContain('<span class="rfc2119 rfc2119-may">MAY</span>');
  });

  it('leaves lowercase must alone', () => {
    const out = renderToHast('The parser must also accept CRLF.');
    expect(out).not.toContain('class="rfc2119');
    expect(out).toContain('must');
  });

  it('does not touch keywords inside inline code', () => {
    const out = renderToHast('Use `MUST` as a keyword.');
    expect(out).not.toContain('class="rfc2119');
  });

  it('does not touch keywords inside fenced code blocks', () => {
    const out = renderToHast('```\nthis MUST stay plain\n```\n');
    expect(out).not.toContain('class="rfc2119');
  });
});
