import type { Root, Element, Text, Parent } from 'hast';
import { visit } from 'unist-util-visit';

export interface CsPermalinksOptions {
  // Set of valid CS-NNNN identifiers harvested from D-changes.mdx.
  knownIds: Set<string>;
  // Path of the file currently being rendered, relative to content root.
  // When this ends in "D-changes.mdx", the plugin is in "add IDs" mode;
  // otherwise it's in "link references" mode.
  // Optional here because Task 33 reads it from the VFile instead.
  filePath?: string;
  // Href of the changes page, e.g. "/appendix/d-changes/".
  changesHref: string;
}

const CS_RE = /\bCS-[0-9]{4}\b/g;

function isInsideTag(ancestors: Element[], tagNames: string[]): boolean {
  return ancestors.some(a => tagNames.includes(a.tagName));
}

export function rehypeCsPermalinks(options: CsPermalinksOptions) {
  const { knownIds, changesHref } = options;

  return function transformer(tree: Root, file?: { path?: string }) {
    // filePath comes from options (Tasks 27-28 tests) or from the VFile (Task 33 runtime).
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

    // Mode B: link free-standing CS-NNNN references. Walk text nodes; skip
    // those inside <a>, <code>, and <pre>.
    const ancestorStack: Element[] = [];
    visit(
      tree,
      (node, index, parent) => {
        if (node.type === 'element') {
          ancestorStack.push(node as Element);
        }
        if (node.type === 'text' && parent && index != null) {
          const text = node as Text;
          if (isInsideTag(ancestorStack, ['a', 'code', 'pre'])) return;

          const matches = [...text.value.matchAll(CS_RE)];
          if (matches.length === 0) return;

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
              newChildren.push({ type: 'text', value: text.value.slice(last, m.index) });
            }
            newChildren.push({
              type: 'element',
              tagName: 'a',
              properties: { href: `${changesHref}#${id}` },
              children: [{ type: 'text', value: id }],
            });
            last = m.index! + id.length;
          }
          if (last < text.value.length) {
            newChildren.push({ type: 'text', value: text.value.slice(last) });
          }

          (parent as Parent).children.splice(index, 1, ...newChildren);
        }
      },
      true,
    );
  };
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
