import fs from 'node:fs';
import path from 'node:path';

export interface ClauseInfo {
  // Absolute site-relative URL including the fragment, e.g.
  // "/02-lexical/#lexical.identifiers".
  href: string;
  // The heading's live number, used as rendered link text
  // (e.g. "2.3", "A.4", "5"). Recomputed on every build.
  number: string;
  // The heading's visible text minus the number and the [#slug] marker,
  // used as the xref's `title` attribute. E.g. "Identifiers".
  text: string;
}

// Matches clause-numbered heading text:
//   <NUM>. <TITLE>. [#<slug>]
// where NUM is a digit run or single uppercase letter (plus optional .M[.K]),
// TITLE is any run up to the [#, and slug matches the slug grammar.
// Capture groups:
//   1 = top   (digit run or letter)
//   2 = mid   (digits, optional)
//   3 = bot   (digits, optional)
//   4 = title (non-greedy)
//   5 = slug  (chapter.leaf grammar)
const HEADING_RE =
  /^(?:#+)\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\.\s+(.+?)\s+\[#([a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)?)\]\s*$/gm;

function numberFrom(top: string, mid?: string, bot?: string): string {
  return [top, mid, bot].filter(Boolean).join('.');
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
 * Harvests every clause-numbered heading and returns a map from slug
 * (e.g. "lexical.identifiers") to the cross-file route + live number +
 * title. Consumed by rehype-clause-xrefs at build time.
 *
 * Throws on:
 * - Duplicate slug across any two headings.
 * - Any clause-numbered heading that lacks a [#slug] marker.
 */
export function harvestClauses(contentRoot: string): Map<string, ClauseInfo> {
  const map = new Map<string, ClauseInfo>();
  const seenAt = new Map<string, string>();

  // First pass: collect slugged headings.
  const files: string[] = [];
  walkMdx(contentRoot, files);

  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const route = fileToRoute(rel);
    const src = fs.readFileSync(abs, 'utf8');

    for (const m of src.matchAll(HEADING_RE)) {
      const [, top, mid, bot, title, slug] = m;
      const number = numberFrom(top, mid, bot);
      if (seenAt.has(slug)) {
        throw new Error(
          `duplicate slug "${slug}": first seen at ${seenAt.get(slug)}, also at ${rel}`,
        );
      }
      seenAt.set(slug, rel);
      map.set(slug, {
        href: `${route}#${slug}`,
        number,
        text: title.trim(),
      });
    }
  }

  // Second pass: detect numbered headings missing a slug marker.
  // A heading line starts with #+ space, then clause grammar, then period.
  // If it lacks `[#...]` trailing, that's a build error.
  const numberedNoSlug = /^#+\s+(?:[0-9]+|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\.\s+(.+)$/gm;
  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const src = fs.readFileSync(abs, 'utf8');
    for (const m of src.matchAll(numberedNoSlug)) {
      const rest = m[1];
      if (!/\[#[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)?\]\s*$/.test(rest)) {
        throw new Error(
          `numbered heading without [#slug] marker in ${rel}: "${m[0]}"`,
        );
      }
    }
  }

  return map;
}

export function defaultContentRoot(projectRoot: string): string {
  return path.join(projectRoot, 'src/content/docs');
}
