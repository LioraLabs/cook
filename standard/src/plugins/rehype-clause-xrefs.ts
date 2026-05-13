import type { Root, Element, Text, Parent } from 'hast';
import type { ClauseInfo } from './clauses.ts';
import { resolveRename } from '../../scripts/slug-renames.ts';
import { SLUG_MAPPING } from '../../scripts/slug-mapping.ts';

// Slugs registered in the mapping but not yet anchored by a chapter file
// during the v0.10 reorg show up here. The cascade below treats them as
// expected forward references and emits a `[upcoming-slug]` warn rather
// than throwing, so Tasks 2–7 can land incrementally.
const KNOWN_SLUGS = new Set(Object.values(SLUG_MAPPING));

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
          // Retired or renamed slug. Emit a precise diagnostic naming the new
          // slug (if any). During the v0.10 transition we `console.warn` rather
          // than throw; Task 8 activates the vitest source-lint as the hard CI
          // gate (see src/plugins/__tests__/no-retired-slugs-in-source.test.ts).
          const rename = resolveRename(slug);
          if (rename === null) {
            console.warn(
              `[slug-renames] §{${slug}} references a slug that was retired with no replacement. See scripts/slug-renames.ts and Cook Standard v0.10 reorg.`,
            );
            continue;
          } else if (rename !== undefined) {
            console.warn(
              `[slug-renames] §{${slug}} renamed to §{${rename}} in Cook Standard v0.10. Update the reference in source. See scripts/slug-renames.ts.`,
            );
            continue;
          } else if (KNOWN_SLUGS.has(slug)) {
            // During the v0.10 transition: a forward reference to a slug
            // whose chapter file has not yet been added. Once Task 8
            // completes, every KNOWN_SLUG either lives in clauseMap or
            // is unreferenced — at which point this branch can be
            // re-tightened back to a throw.
            console.warn(
              `[upcoming-slug] §{${slug}} resolves to a v0.10 chapter that has not yet landed in MDX. This warning will clear when the chapter file is added.`,
            );
            continue;
          } else {
            // slug absent from clauseMap, renames registry, and the
            // v0.10 mapping — genuine unknown.
            throw new Error(
              `unknown clause slug "${slug}" — §{${slug}} did not resolve in clauseMap`,
            );
          }
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
