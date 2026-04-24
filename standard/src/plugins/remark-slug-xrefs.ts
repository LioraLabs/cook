import type { Root, Text } from 'mdast';
import { visit, SKIP } from 'unist-util-visit';

// Slug grammar: chapter.leaf, lowercase kebab-case, optional namespace dot.
const SLUG_RE = /^[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)?$/;

// MDX parses `§{slug}` as two nodes: a trailing `§` in a text node, followed
// by an mdxTextExpression (or mdxFlowExpression in block position) whose
// `value` is the raw text between the braces. MDX's default behavior is to
// compile the expression to JS, which at runtime throws because `slug` is
// not a defined variable.
//
// This plugin undoes the JSX-expression interpretation whenever the
// expression's value is a valid slug AND the immediately preceding sibling
// text ends with `§`. The pair collapses back into a single plain text node
// `§{slug}`, which the rehype-clause-xrefs plugin then handles.
//
// Non-slug expressions (`{someVar}` elsewhere in the doc) are left alone —
// authors can still write real MDX expressions when they want to.
export function remarkSlugXrefs() {
  return function transformer(tree: Root) {
    visit(tree, (node, index, parent) => {
      if (
        (node.type !== 'mdxTextExpression' && node.type !== 'mdxFlowExpression') ||
        parent == null ||
        typeof index !== 'number'
      ) {
        return;
      }

      // mdxTextExpression / mdxFlowExpression have a `value` property carrying
      // the raw expression source. Not typed in mdast because these are MDX
      // extensions.
      const exprValue = (node as unknown as { value: string }).value;
      if (!SLUG_RE.test(exprValue)) return;

      const prev = parent.children[index - 1];
      if (!prev || prev.type !== 'text') return;
      const prevText = (prev as Text).value;
      if (!prevText.endsWith('§')) return;

      // Collapse `§` + expression into plain text `§{slug}`.
      (prev as Text).value = prevText.slice(0, -1) + `§{${exprValue}}`;
      parent.children.splice(index, 1);
      return [SKIP, index];
    });
  };
}

export default remarkSlugXrefs;
