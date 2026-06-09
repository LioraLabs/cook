import { describe, it, expect } from 'vitest';
import { remark } from 'remark';
import { remarkCookHighlight } from '../remark-cook-highlight';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const WASM_PATH = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '../../..', // → standard/
  'public/tree-sitter-cook.wasm',
);

const QUERY_PATH = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '../../../..', // → repo root
  'tree-sitter-cook/queries/highlights.scm',
);

async function process(input: string): Promise<string> {
  const file = await remark()
    .use(remarkCookHighlight, { wasmPath: WASM_PATH, queryPath: QUERY_PATH })
    .process(input);
  return String(file);
}

describe('remarkCookHighlight', () => {
  it('highlights a fenced `cook` block', async () => {
    const out = await process('```cook\nrecipe build\n```\n');
    // Expect the keyword `recipe` to be wrapped in a span with a class
    // derived from the highlights query's @keyword capture.
    expect(out).toMatch(/<span class="hl-keyword">recipe<\/span>/);
  });

  it('leaves non-cook code blocks alone', async () => {
    const out = await process('```shell\necho hello\n```\n');
    expect(out).not.toContain('hl-keyword');
    expect(out).toContain('echo hello');
  });

  it('preserves text when the cook source is malformed', async () => {
    // Tree-sitter error recovery: we still emit the original text, no crash.
    // (`recipe "x" }{` produces an ERROR node under the current grammar; the
    // former `@#$%` fixture started parsing cleanly once `@` interactive
    // commands and `#` comments landed.)
    const out = await process('```cook\nrecipe "x" }{\n```\n');
    expect(out).toContain('}{');
  });
});
