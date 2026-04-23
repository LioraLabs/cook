import type { Root, Element, Text, Parent } from 'hast';
import type { ClauseInfo } from './clauses.ts';

export interface ClauseXrefsOptions {
  // Map from anchor ID (sec-X-Y-Z) to { href, text }, built by
  // harvestClauses() once per build.
  clauseMap: Map<string, ClauseInfo>;
}

// Matches `§ X[.Y[.Z]]` in prose. The section sign may be followed by one or
// more spaces. X is either a digit run or a single uppercase letter; Y/Z are
// digits. The trailing `\b` keeps matches word-bounded so we don't match into
// a longer token.
const XREF_RE = /§\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\b/g;

const SKIP_TAGS = new Set(['a', 'code', 'pre']);

function clauseIdFrom(top: string, mid?: string, bot?: string): string {
  const parts = [top, mid, bot].filter(Boolean) as string[];
  return `sec-${parts.join('-')}`;
}

export function rehypeClauseXrefs(options: ClauseXrefsOptions) {
  const { clauseMap } = options;

  return function transformer(tree: Root) {
    // Two-pass walk (same pattern as rehype-cs-permalinks): collect eligible
    // text nodes with correct enter/leave ancestor tracking, then splice
    // replacements in reverse document order so mutations don't invalidate
    // earlier indices.
    const targets: Array<{ parent: Parent; index: number; value: string }> = [];
    collectTextNodes(tree, [], targets);

    for (let t = targets.length - 1; t >= 0; t--) {
      const { parent, index, value } = targets[t];
      const matches = [...value.matchAll(XREF_RE)];
      if (matches.length === 0) continue;

      // Only splice if at least one match resolves. Unresolved refs
      // (e.g., `§ N` meta-placeholders that look like real refs but
      // don't correspond to an actual clause) are left as plain text.
      const resolved = matches.filter(m => {
        const id = clauseIdFrom(m[1], m[2], m[3]);
        return clauseMap.has(id);
      });
      if (resolved.length === 0) continue;

      const newChildren: Array<Text | Element> = [];
      let last = 0;
      for (const m of matches) {
        const full = m[0];
        const id = clauseIdFrom(m[1], m[2], m[3]);
        const info = clauseMap.get(id);

        if (m.index! > last) {
          newChildren.push({ type: 'text', value: value.slice(last, m.index) });
        }
        if (info) {
          newChildren.push({
            type: 'element',
            tagName: 'a',
            properties: {
              href: info.href,
              className: ['clause-xref'],
              title: info.text,
            },
            children: [{ type: 'text', value: full }],
          });
        } else {
          // Keep unresolved refs as plain text — do not error.
          newChildren.push({ type: 'text', value: full });
        }
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
