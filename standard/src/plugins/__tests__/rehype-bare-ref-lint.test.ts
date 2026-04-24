import { describe, it, expect } from 'vitest';
import { rehype } from 'rehype';
import { rehypeBareRefLint } from '../rehype-bare-ref-lint';

function process(html: string): string {
  return String(
    rehype()
      .data('settings', { fragment: true })
      .use(rehypeBareRefLint)
      .processSync(html),
  );
}

describe('rehypeBareRefLint', () => {
  it('passes clean prose with no § refs', () => {
    expect(() => process('<p>Nothing to see here.</p>')).not.toThrow();
  });

  it('passes prose with §{slug} refs', () => {
    expect(() =>
      process('<p>See §{lexical.identifiers}.</p>'),
    ).not.toThrow();
  });

  it('throws on bare § 2.3', () => {
    expect(() => process('<p>See § 2.3.</p>')).toThrowError(
      /bare numeric § reference "§ 2\.3"/i,
    );
  });

  it('throws on bare § N.M.K', () => {
    expect(() => process('<p>See § 4.6.2 for more.</p>')).toThrowError(
      /bare numeric § reference "§ 4\.6\.2"/i,
    );
  });

  it('throws on bare § A.3 (appendix)', () => {
    expect(() => process('<p>See § A.3.</p>')).toThrowError(
      /bare numeric § reference "§ A\.3"/i,
    );
  });

  it('throws on bare § 5 (chapter)', () => {
    expect(() => process('<p>See § 5.</p>')).toThrowError(
      /bare numeric § reference "§ 5"/i,
    );
  });

  it('ignores § inside inline code', () => {
    expect(() =>
      process('<p>The token <code>§ 2.3</code> is literal.</p>'),
    ).not.toThrow();
  });

  it('ignores § inside <pre> blocks', () => {
    expect(() =>
      process('<pre><code>§ 2.3</code></pre>'),
    ).not.toThrow();
  });

  it('ignores § inside existing <a> (already-linked legacy)', () => {
    expect(() =>
      process('<p><a href="/x">§ 2.3</a></p>'),
    ).not.toThrow();
  });
});
