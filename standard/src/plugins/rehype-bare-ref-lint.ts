import type { Root, Element, Text, Parent } from 'hast';

// Matches bare `§ N[.M[.K]]` or `§ X[.N[.M]]` (X = single uppercase letter).
// Leading `§ ` includes a literal space; the ref ends at a word boundary so
// we don't match mid-identifier.
const BARE_REF_RE = /§\s+([0-9]+|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\b/g;

const SKIP_TAGS = new Set(['a', 'code', 'pre']);

export function rehypeBareRefLint() {
  return function transformer(tree: Root) {
    const hits: string[] = [];
    walk(tree, [], hits);
    if (hits.length > 0) {
      const list = hits.map(h => `  bare numeric § reference "${h}"`).join('\n');
      throw new Error(
        `bare numeric § reference(s) found in prose — use §{chapter.leaf} slug form instead:\n${list}`,
      );
    }
  };
}

function walk(node: Root | Element, ancestors: Element[], out: string[]): void {
  const parent = node as unknown as Parent;
  for (const child of parent.children) {
    if (child.type === 'text') {
      const skip = ancestors.some(a => SKIP_TAGS.has(a.tagName));
      if (skip) continue;
      for (const m of (child as Text).value.matchAll(BARE_REF_RE)) {
        out.push(m[0]);
      }
    } else if (child.type === 'element') {
      ancestors.push(child as Element);
      walk(child as Element, ancestors, out);
      ancestors.pop();
    }
  }
}

export default rehypeBareRefLint;
