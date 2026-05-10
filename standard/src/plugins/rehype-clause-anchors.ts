import type { Root, Element, Text } from 'hast';
import { visit } from 'unist-util-visit';

// Clause-number prefix at start of heading text. N is a digit run or a
// single uppercase letter; optional .M[.K] subsection numbers.
const CLAUSE_RE = /^([0-9]+|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\.(\s|$)/;

// Trailing [#slug] marker. Slug grammar: chapter.leaf with dash-separated
// words; one or more dotted segments (bare chapter also accepted).
const MARKER_RE = /\s+\[#([^\]]+)\]\s*$/;
const SLUG_RE = /^[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)*$/;

function extractText(el: Element): string {
  let out = '';
  for (const child of el.children) {
    if (child.type === 'text') out += child.value;
    else if (child.type === 'element') out += extractText(child as Element);
  }
  return out;
}

function stripMarkerFromLastText(el: Element): void {
  // Walk children in reverse; the marker lives in the trailing text node.
  for (let i = el.children.length - 1; i >= 0; i--) {
    const c = el.children[i];
    if (c.type === 'text') {
      const t = c as Text;
      const m = t.value.match(MARKER_RE);
      if (m) {
        t.value = t.value.slice(0, m.index);
      }
      return;
    }
    if (c.type === 'element') {
      stripMarkerFromLastText(c as Element);
      return;
    }
  }
}

export function rehypeClauseAnchors() {
  return function transformer(tree: Root) {
    const seen = new Map<string, string>();
    visit(tree, 'element', (node: Element) => {
      if (!/^h[1-6]$/.test(node.tagName)) return;
      const text = extractText(node);
      if (!CLAUSE_RE.test(text)) return;

      const markerMatch = text.match(MARKER_RE);
      if (!markerMatch) {
        throw new Error(
          `clause-numbered heading missing [#slug] marker: "${text}"`,
        );
      }
      const slug = markerMatch[1];
      if (!SLUG_RE.test(slug)) {
        throw new Error(
          `invalid slug "${slug}" in heading "${text}" — must match [a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)*`,
        );
      }
      if (seen.has(slug)) {
        throw new Error(
          `duplicate slug "${slug}": appears in both "${seen.get(slug)}" and "${text}"`,
        );
      }
      seen.set(slug, text);

      stripMarkerFromLastText(node);
      if (MARKER_RE.test(extractText(node))) {
        throw new Error(
          `heading "${text}": [#slug] marker could not be stripped (wrapped in an inline element?)`,
        );
      }
      node.properties = node.properties ?? {};
      (node.properties as Record<string, unknown>).id = slug;
    });
  };
}

export default rehypeClauseAnchors;
