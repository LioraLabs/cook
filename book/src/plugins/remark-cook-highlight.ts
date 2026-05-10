import type { Root, Code } from 'mdast';
import { visit } from 'unist-util-visit';
import fs from 'node:fs/promises';
import { Parser, Language, Query } from 'web-tree-sitter';

type TsParser = InstanceType<typeof Parser>;
type TsQuery = InstanceType<typeof Query>;

export interface CookHighlightOptions {
  wasmPath: string;
  queryPath: string;
}

const CAPTURE_TO_CLASS: Record<string, string> = {
  keyword:              'hl-keyword',
  'function':           'hl-function',
  'function.builtin':   'hl-function-builtin',
  module:               'hl-module',
  string:               'hl-string',
  number:               'hl-number',
  comment:              'hl-comment',
  operator:             'hl-operator',
  punctuation:          'hl-punctuation',
  variable:             'hl-variable',
  identifier:           'hl-identifier',
};

let parserPromise: Promise<{ parser: TsParser; query: TsQuery }> | null = null;

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

async function getParser(opts: CookHighlightOptions) {
  if (parserPromise) return parserPromise;
  parserPromise = (async () => {
    await Parser.init();
    const parser = new Parser();
    const wasmBytes = await fs.readFile(opts.wasmPath);
    const language = await Language.load(wasmBytes);
    parser.setLanguage(language);
    const queryText = await fs.readFile(opts.queryPath, 'utf8');
    const query = new Query(language, queryText);
    return { parser, query };
  })();
  return parserPromise;
}

function renderHighlighted(source: string, parser: TsParser, query: TsQuery): string {
  const tree = parser.parse(source);
  if (!tree) return escapeHtml(source);
  const captures = query.captures(tree.rootNode);

  type Range = { start: number; end: number; className: string };
  const rangesByStart = new Map<number, Range>();
  for (const cap of captures) {
    const cls = CAPTURE_TO_CLASS[cap.name];
    if (!cls) continue;
    const start = cap.node.startIndex;
    const end = cap.node.endIndex;
    const existing = rangesByStart.get(start);
    if (!existing || (end - start) > (existing.end - existing.start)) {
      rangesByStart.set(start, { start, end, className: cls });
    }
  }
  const ranges = [...rangesByStart.values()].sort((a, b) => a.start - b.start);

  let out = '';
  let cursor = 0;
  for (const r of ranges) {
    if (r.start < cursor) continue;
    if (r.start > cursor) out += escapeHtml(source.slice(cursor, r.start));
    out += `<span class="${r.className}">${escapeHtml(source.slice(r.start, r.end))}</span>`;
    cursor = r.end;
  }
  if (cursor < source.length) out += escapeHtml(source.slice(cursor));
  return out;
}

export function remarkCookHighlight(options: CookHighlightOptions) {
  return async function transformer(tree: Root) {
    const pending: Code[] = [];
    visit(tree, 'code', (node: Code) => {
      if (node.lang === 'cook') pending.push(node);
    });
    if (pending.length === 0) return;

    const { parser, query } = await getParser(options);

    for (const node of pending) {
      const highlighted = renderHighlighted(node.value, parser, query);
      (node as any).type = 'html';
      (node as any).value = `<pre class="cook-block"><code class="language-cook">${highlighted}</code></pre>`;
    }
  };
}

export default remarkCookHighlight;
