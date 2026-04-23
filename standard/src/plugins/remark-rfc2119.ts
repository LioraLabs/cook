import type { Root, Text, Parent } from 'mdast';
import { visit } from 'unist-util-visit';

// Order matters: two-word keywords MUST be matched before their single-word
// prefixes, otherwise "MUST" will swallow the "MUST" in "MUST NOT".
const KEYWORD_PATTERNS: Array<{ match: RegExp; className: string; label: string }> = [
  { match: /\bMUST NOT\b/g,   className: 'rfc2119-must',       label: 'MUST NOT' },
  { match: /\bSHALL NOT\b/g,  className: 'rfc2119-shall',      label: 'SHALL NOT' },
  { match: /\bSHOULD NOT\b/g, className: 'rfc2119-should',     label: 'SHOULD NOT' },
  { match: /\bMAY NOT\b/g,    className: 'rfc2119-may',        label: 'MAY NOT' },
  { match: /\bMUST\b/g,       className: 'rfc2119-must',       label: 'MUST' },
  { match: /\bSHALL\b/g,      className: 'rfc2119-shall',      label: 'SHALL' },
  { match: /\bSHOULD\b/g,     className: 'rfc2119-should',     label: 'SHOULD' },
  { match: /\bMAY\b/g,        className: 'rfc2119-may',        label: 'MAY' },
  { match: /\bREQUIRED\b/g,   className: 'rfc2119-required',   label: 'REQUIRED' },
  { match: /\bRECOMMENDED\b/g, className: 'rfc2119-recommended', label: 'RECOMMENDED' },
  { match: /\bOPTIONAL\b/g,   className: 'rfc2119-optional',   label: 'OPTIONAL' },
];

type Replacement =
  | { kind: 'text'; value: string }
  | { kind: 'html'; value: string };

function splitOnKeywords(value: string): Replacement[] | null {
  // Fast path: no keyword characters, bail out.
  if (!/[A-Z]{3}/.test(value)) return null;

  // Build a single combined regex in the defined order.
  const combined = new RegExp(
    KEYWORD_PATTERNS.map(p => `(?:${p.match.source.replace(/\\b/g, '\\b')})`).join('|'),
    'g',
  );

  let out: Replacement[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  combined.lastIndex = 0;
  while ((m = combined.exec(value)) !== null) {
    if (m.index > last) {
      out.push({ kind: 'text', value: value.slice(last, m.index) });
    }
    const hit = m[0];
    const pat = KEYWORD_PATTERNS.find(p => new RegExp(`^${p.match.source.replace(/\\b/g, '')}$`).test(hit));
    if (!pat) {
      // Shouldn't happen, but if it does, leave the text alone.
      out.push({ kind: 'text', value: hit });
    } else {
      out.push({
        kind: 'html',
        value: `<span class="rfc2119 ${pat.className}">${pat.label}</span>`,
      });
    }
    last = m.index + hit.length;
  }
  if (last === 0) return null;
  if (last < value.length) {
    out.push({ kind: 'text', value: value.slice(last) });
  }
  return out;
}

export function remarkRfc2119() {
  return function transformer(tree: Root) {
    visit(tree, 'text', (node: Text, index, parent: Parent | undefined) => {
      if (!parent || index == null) return;
      // Skip text nodes inside code-ish contexts. `inlineCode` is skipped by
      // visitor type (it's a different node); but a `text` child of `code`
      // doesn't exist — code blocks are single `code` nodes with a raw value.
      // So we only need to worry about the parent being a heading/para/etc.
      const replacements = splitOnKeywords(node.value);
      if (!replacements) return;

      const newChildren: any[] = replacements.map(r =>
        r.kind === 'text'
          ? { type: 'text', value: r.value }
          : { type: 'html', value: r.value },
      );

      parent.children.splice(index, 1, ...newChildren);
      return index + newChildren.length;
    });
  };
}

export default remarkRfc2119;
