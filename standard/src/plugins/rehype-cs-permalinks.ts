import type { Root, Element, Text, Parent } from 'hast';
import { visit } from 'unist-util-visit';

export interface CsPermalinksOptions {
  // Set of valid CS-NNNN identifiers harvested from D-changes.mdx.
  knownIds: Set<string>;
  // Href of the changes page, e.g. "/appendix/d-changes/".
  changesHref: string;
  // When provided, this overrides the VFile.path used at runtime. Primarily
  // useful for unit tests that do not construct a full VFile pipeline.
  filePath?: string;
}

const CS_RE = /\bCS-[0-9]{4}\b/g;
const SKIP_TAGS = new Set(['a', 'code', 'pre']);

export function rehypeCsPermalinks(options: CsPermalinksOptions) {
  const { knownIds, changesHref } = options;

  return function transformer(tree: Root, file?: { path?: string }) {
    const filePath = options.filePath ?? file?.path ?? '';
    const isChangesFile = /D-changes\.mdx$/.test(filePath);

    if (isChangesFile) {
      // Mode A: stamp IDs on headings that begin with "CS-NNNN".
      visit(tree, 'element', (node: Element) => {
        if (!/^h[1-6]$/.test(node.tagName)) return;
        const text = extractText(node);
        const m = text.match(/^\s*(CS-[0-9]{4})\b/);
        if (!m) return;
        node.properties = node.properties ?? {};
        (node.properties as Record<string, unknown>).id = m[1];
      });
      return;
    }

    // Mode B: link free-standing CS-NNNN references. We collect all eligible
    // text nodes in a single depth-first pass (so we can correctly track
    // ancestors) and then splice replacements in reverse document order so
    // earlier indices stay valid as we mutate each parent.
    const targets: Array<{ parent: Parent; index: number; value: string }> = [];
    collectTextNodes(tree, [], targets);

    for (let t = targets.length - 1; t >= 0; t--) {
      const { parent, index, value } = targets[t];
      const matches = [...value.matchAll(CS_RE)];
      if (matches.length === 0) continue;

      const newChildren: Array<Text | Element> = [];
      let last = 0;
      for (const m of matches) {
        const id = m[0];
        if (!knownIds.has(id)) {
          throw new Error(
            `"${id}" is a dangling CS-NNNN reference in ${filePath}: no matching entry in D-changes.mdx`,
          );
        }
        if (m.index! > last) {
          newChildren.push({ type: 'text', value: value.slice(last, m.index) });
        }
        newChildren.push({
          type: 'element',
          tagName: 'a',
          properties: { href: `${changesHref}#${id}`, className: ['cs-link'] },
          children: [{ type: 'text', value: id }],
        });
        last = m.index! + id.length;
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

function extractText(el: Element): string {
  let out = '';
  for (const child of el.children) {
    if (child.type === 'text') out += child.value;
    else if (child.type === 'element') out += extractText(child);
  }
  return out;
}

export default rehypeCsPermalinks;
