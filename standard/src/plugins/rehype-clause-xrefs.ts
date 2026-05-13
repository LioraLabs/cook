import type { Root, Element, Text, Parent } from 'hast';
import type { ClauseInfo } from './clauses.ts';
import { resolveRename } from '../../scripts/slug-renames.ts';

export interface ClauseXrefsOptions {
  clauseMap: Map<string, ClauseInfo>;
}

// Matches `§{chapter.leaf}` in prose. The braces delimit the slug so no
// word-boundary heuristics are needed. Slug characters: lowercase letters,
// digits, dash, and a single dot separating chapter from leaf.
const XREF_RE = /§\{([a-z0-9.-]+)\}/g;

const SKIP_TAGS = new Set(['a', 'code', 'pre']);

export function rehypeClauseXrefs(options: ClauseXrefsOptions) {
  const { clauseMap } = options;

  return function transformer(tree: Root) {
    const targets: Array<{ parent: Parent; index: number; value: string }> = [];
    collectTextNodes(tree, [], targets);

    for (let t = targets.length - 1; t >= 0; t--) {
      const { parent, index, value } = targets[t];
      const matches = [...value.matchAll(XREF_RE)];
      if (matches.length === 0) continue;

      const newChildren: Array<Text | Element> = [];
      let last = 0;
      for (const m of matches) {
        const full = m[0];
        const slug = m[1];
        const info = clauseMap.get(slug);
        if (!info) {
          // Before emitting a generic "unresolved" error, consult the
          // slug-renames registry. If the slug is a retired one introduced
          // by the v0.10 structural redesign, surface a precise rename
          // diagnostic naming the new slug.
          const rename = resolveRename(slug);
          if (rename === null) {
            throw new Error(
              `§{${slug}} references a slug that was retired with no ` +
              `replacement. See scripts/slug-renames.ts and Cook Standard ` +
              `v0.10 reorg.`,
            );
          }
          if (rename !== undefined) {
            throw new Error(
              `§{${slug}} renamed to §{${rename}} in Cook Standard v0.10. ` +
              `Update the reference in source. See scripts/slug-renames.ts.`,
            );
          }
          throw new Error(
            `unknown clause slug "${slug}" — §{${slug}} did not resolve in clauseMap`,
          );
        }

        if (m.index! > last) {
          newChildren.push({ type: 'text', value: value.slice(last, m.index) });
        }
        newChildren.push({
          type: 'element',
          tagName: 'a',
          properties: {
            href: info.href,
            className: ['clause-xref'],
            title: `${info.number}. ${info.text}`,
          },
          children: [{ type: 'text', value: `§ ${info.number}` }],
        });
        last = m.index! + full.length;
      }
      if (last < value.length) {
        newChildren.push({ type: 'text', value: value.slice(last) });
      }

      parent.children.splice(index, 1, ...newChildren);
    }
  };
}

function collectTextNodes(
  node: Root | Element,
  ancestors: Element[],
  out: Array<{ parent: Parent; index: number; value: string }>,
): void {
  const parent = node as unknown as Parent;
  for (let i = 0; i < parent.children.length; i++) {
    const child = parent.children[i];
    if (child.type === 'text') {
      const skip = ancestors.some(a => SKIP_TAGS.has(a.tagName));
      if (!skip) {
        out.push({ parent, index: i, value: (child as Text).value });
      }
    } else if (child.type === 'element') {
      ancestors.push(child as Element);
      collectTextNodes(child as Element, ancestors, out);
      ancestors.pop();
    }
  }
}

export default rehypeClauseXrefs;
