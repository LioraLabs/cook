import { describe, it, expect } from 'vitest';
import { rehype } from 'rehype';
import { VFile } from 'vfile';
import { rehypeCsPermalinks } from '../rehype-cs-permalinks';

function process(html: string, opts?: { knownIds?: Set<string>; filePath?: string }): string {
  const vfile = new VFile({ value: html, path: opts?.filePath ?? '02-lexical.mdx' });
  return String(
    rehype()
      .data('settings', { fragment: true })
      .use(rehypeCsPermalinks, {
        knownIds: opts?.knownIds ?? new Set(['CS-0001', 'CS-0002', 'CS-0007']),
        changesHref: '/appendix/d-changes/',
      })
      .processSync(vfile),
  );
}

describe('rehypeCsPermalinks', () => {
  it('links a free-standing CS-NNNN reference to the changes page', () => {
    const out = process('<p>See CS-0007 for context.</p>');
    expect(out).toContain('href="/appendix/d-changes/#CS-0007"');
    expect(out).toContain('class="cs-link"');
    expect(out).toContain('>CS-0007</a>');
  });

  it('does not wrap CS-NNNN that is already inside a link', () => {
    const out = process('<p>See <a href="/elsewhere">CS-0001</a>.</p>');
    expect(out).toContain('<a href="/elsewhere">CS-0001</a>');
    expect(out).not.toContain('href="/appendix/d-changes/#CS-0001"');
  });

  it('does not wrap CS-NNNN inside inline code', () => {
    const out = process('<p>The token <code>CS-0001</code> is literal.</p>');
    expect(out).toContain('<code>CS-0001</code>');
    expect(out).not.toContain('href="/appendix/d-changes/#CS-0001"');
  });

  it('links a CS-NNNN that follows a closed <a> in the same paragraph', () => {
    // Regression test: the previous implementation tracked ancestors with a
    // push-only stack, so after traversing into a sibling <a> it would still
    // treat subsequent text siblings as "inside <a>" and skip them.
    const out = process(
      '<p>See <a href="/x">some link</a> and CS-0007 for context.</p>',
    );
    expect(out).toContain('<a href="/x">some link</a>');
    expect(out).toContain('href="/appendix/d-changes/#CS-0007"');
    expect(out).toContain('>CS-0007</a>');
  });

  it('throws on a dangling CS-NNNN reference', () => {
    expect(() =>
      process('<p>See CS-9999 for context.</p>'),
    ).toThrowError(/CS-9999.*dangling/i);
  });

  it('when processing E-changes itself, adds id attributes to entry anchors', () => {
    const out = process(
      '<h3>CS-0007 — Multi-output cook</h3>',
      { filePath: 'appendix/E-changes.mdx' },
    );
    expect(out).toContain('id="CS-0007"');
  });
});
