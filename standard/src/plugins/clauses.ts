import fs from 'node:fs';
import path from 'node:path';

export interface ClauseInfo {
  // Absolute site-relative URL including the fragment, e.g.
  // "/02-lexical/#sec-2-3" or "/appendix/a-grammar/#sec-A-3".
  href: string;
  // The heading's visible text, used as the xref's `title` attribute
  // (browser-native tooltip).
  text: string;
}

// Matches clause-numbered headings: `NN. Title` where NN is a digit run or a
// single uppercase letter, with optional `.M[.K]` subsection numbers. Must be
// followed by a period then whitespace or end-of-line. Mirrors CLAUSE_RE in
// rehype-clause-anchors.ts.
const HEADING_RE = /^(#+)\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\.(\s|$)(.*)$/gm;

function clauseIdFrom(top: string, mid?: string, bot?: string): string {
  const parts = [top, mid, bot].filter(Boolean) as string[];
  return `sec-${parts.join('-')}`;
}

// Starlight lowercases file slugs. `02-lexical.mdx` → `/02-lexical/`;
// `appendix/A-grammar.mdx` → `/appendix/a-grammar/`.
function fileToRoute(relPath: string): string {
  const noExt = relPath.replace(/\.mdx$/, '').toLowerCase();
  if (noExt === 'index') return '/';
  return `/${noExt}/`;
}

function walkMdx(root: string, out: string[]): void {
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const p = path.join(root, entry.name);
    if (entry.isDirectory()) walkMdx(p, out);
    else if (entry.isFile() && entry.name.endsWith('.mdx')) out.push(p);
  }
}

/**
 * Harvests every clause-numbered heading from the Starlight content
 * collection and returns a map from anchor ID (sec-X-Y-Z) to the
 * cross-file route + heading text needed to build an xref link.
 *
 * Consumed by rehype-clause-xrefs at build time to auto-link `§ X.Y[.Z]`
 * references to their definitions regardless of which chapter the
 * reference lives in.
 */
export function harvestClauses(contentRoot: string): Map<string, ClauseInfo> {
  const map = new Map<string, ClauseInfo>();
  const files: string[] = [];
  walkMdx(contentRoot, files);

  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const route = fileToRoute(rel);
    const src = fs.readFileSync(abs, 'utf8');

    for (const m of src.matchAll(HEADING_RE)) {
      const [, , top, mid, bot, , rest] = m;
      const id = clauseIdFrom(top, mid, bot);
      const title = rest.trim();
      // Reconstruct the full visible heading text (number + title) to use
      // as the tooltip body. rest captures only what follows the trailing
      // period.
      const numeric = [top, mid, bot].filter(Boolean).join('.');
      const text = title ? `${numeric}. ${title}` : numeric;
      // First writer wins — in practice ids are unique; the clause-anchors
      // plugin fails the build if they aren't.
      if (!map.has(id)) {
        map.set(id, { href: `${route}#${id}`, text });
      }
    }
  }

  return map;
}

export function defaultContentRoot(projectRoot: string): string {
  return path.join(projectRoot, 'src/content/docs');
}
