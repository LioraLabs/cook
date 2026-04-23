import type { Root, Element } from 'hast';
import { visit } from 'unist-util-visit';

// Matches `N[.M[.K]].` at the start of a clause heading, where N is either a
// digit (normative chapters) or a single uppercase letter (appendices), and
// M/K are digits. Normative chapters use `## 2.1. Title` style (trailing
// period, no section sign). The trailing period is required so that stray
// numeric prefixes like "Note 2.1.1 ..." on a non-clause heading don't match
// — but "Note" starts with a letter anyway, so the `^` anchor alone is enough.
// The required trailing `.` combined with whitespace-or-EOL after it also
// excludes things like "Appendix A. Grammar (normative)" (which does match
// and produces sec-A — that's intentional, it's the appendix's own anchor).
const CLAUSE_RE = /^([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\.(\s|$)/;

function extractText(el: Element): string {
  let out = '';
  for (const child of el.children) {
    if (child.type === 'text') out += child.value;
    else if (child.type === 'element') out += extractText(child);
  }
  return out;
}

function clauseIdFrom(match: RegExpMatchArray): string {
  const parts = [match[1], match[2], match[3]].filter(Boolean);
  return `sec-${parts.join('-')}`;
}

export function rehypeClauseAnchors() {
  return function transformer(tree: Root) {
    const seen = new Map<string, string>();
    visit(tree, 'element', (node: Element) => {
      if (!/^h[1-6]$/.test(node.tagName)) return;
      const text = extractText(node);
      const m = text.match(CLAUSE_RE);
      if (!m) return;
      const id = clauseIdFrom(m);
      if (seen.has(id)) {
        throw new Error(
          `duplicate clause number: "${id}" appears in both "${seen.get(id)}" and "${text}"`,
        );
      }
      seen.set(id, text);
      node.properties = node.properties ?? {};
      (node.properties as Record<string, unknown>).id = id;
    });
  };
}

export default rehypeClauseAnchors;
