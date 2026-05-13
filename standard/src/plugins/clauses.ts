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

// Canonical slug grammar: chapter.leaf or bare word, all lowercase-kebab.
const SLUG_SHAPE_RE = /^[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)*$/;

// Matches clause-numbered heading text that carries a valid [#slug] marker:
//   <NUM>. <TITLE> [#<slug>]                  ← canonical form
//   Note <NUM> — <TITLE> [#<slug>]            ← note form (em-dash separator)
// where NUM is a digit run (optionally followed by a single lowercase letter,
// e.g. "4a" for interstitial chapters), or a single uppercase letter (appendix),
// with an optional .M[.K] numeric sub-number. TITLE is any run up to the [#,
// and slug matches SLUG_SHAPE_RE.
// Capture groups:
//   1 = top   (digit run + optional letter, or single uppercase letter)
//   2 = mid   (digits, optional)
//   3 = bot   (digits, optional)
//   4 = title (non-greedy)
//   5 = slug  (chapter.leaf grammar — one or more dotted segments)
const HEADING_RE =
  /^(?:#+)\s+(?:Note\s+)?([0-9]+[a-z]?|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?(?:\.|\s+—)\s+(.+?)\s+\[#([a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)*)\]\s*$/gm;

// Matches any clause-numbered heading line regardless of whether it has a marker.
// Capture group 1 = everything after the number-and-period prefix.
//
// Intentionally strict: does NOT match the "Note <NUM> — TITLE" form because
// notes without slug markers (the common case) are allowed to be link-free —
// requiring a slug on every note would force inventing slugs for purely
// expository asides. The harvester (HEADING_RE above) DOES match the note
// form when a slug marker is present, so notes that need to be xref'd can
// opt in by adding [#slug].
const NUMBERED_HEADING_RE =
  /^#+\s+(?:[0-9]+[a-z]?|[A-Z])(?:\.[0-9]+(?:\.[0-9]+)?)?\.\s+(.+)$/gm;

// Matches a trailing [#<anything>] marker (present but possibly invalid slug).
const MARKER_PRESENT_RE = /\[#([^\]]+)\]\s*$/;

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

// Strip fenced code blocks from MDX source so that heading-like lines inside
// them are not matched by either pass. Line count is preserved (blanked lines
// replace fenced content) so error messages stay line-accurate if added later.
function stripFencedCode(src: string): string {
  const out: string[] = [];
  let inFence = false;
  for (const line of src.split('\n')) {
    if (/^```/.test(line.trimStart())) {
      inFence = !inFence;
      out.push(''); // preserve line count
      continue;
    }
    out.push(inFence ? '' : line);
  }
  return out.join('\n');
}

// Pass 1: collect every heading that already carries a valid [#slug] marker.
function harvestSluggedHeadings(
  files: string[],
  contentRoot: string,
): Map<string, ClauseInfo> {
  const map = new Map<string, ClauseInfo>();
  const seenAt = new Map<string, string>();

  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const route = fileToRoute(rel);
    const src = stripFencedCode(fs.readFileSync(abs, 'utf8'));

    for (const m of src.matchAll(HEADING_RE)) {
      const [, top, mid, bot, title, slug] = m;
      const number = numberFrom(top, mid, bot);
      if (seenAt.has(slug)) {
        // During v0.10 transition: warn rather than throw on duplicate slugs.
        // New chapter files (per the v0.10 reorg) coexist with legacy files
        // until Task 8 deletes the legacy ones. The later occurrence wins.
        // See plans/2026-05-13-standard-reorg-plan.md.
        console.warn(
          `[clauses] duplicate slug "${slug}": first seen at ${seenAt.get(slug)}, also at ${rel}. Preferring the later occurrence during the v0.10 transition. Task 8 deletes the legacy file; the strict gate then re-asserts.`,
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

  return map;
}

// Pass 2: assert that every clause-numbered heading has a valid [#slug] marker.
// Distinguishes "marker absent" from "marker present but slug grammar invalid".
function assertAllNumberedHeadingsSlugged(
  files: string[],
  contentRoot: string,
): void {
  for (const abs of files) {
    const rel = path.relative(contentRoot, abs);
    const src = stripFencedCode(fs.readFileSync(abs, 'utf8'));

    for (const m of src.matchAll(NUMBERED_HEADING_RE)) {
      const heading = m[0];
      const rest = m[1];
      const markerMatch = MARKER_PRESENT_RE.exec(rest);

      if (!markerMatch) {
        throw new Error(
          `numbered heading without [#slug] marker in ${rel}: "${heading}"`,
        );
      }

      const slug = markerMatch[1];
      if (!SLUG_SHAPE_RE.test(slug)) {
        throw new Error(
          `invalid slug "${slug}" in ${rel}: "${heading}" — expected [a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)*`,
        );
      }
    }
  }
}

/**
 * Harvests every clause-numbered heading and returns a map from slug
 * (e.g. "lexical.identifiers") to the cross-file route + live number +
 * title. Consumed by rehype-clause-xrefs at build time.
 *
 * Throws on:
 * - Any clause-numbered heading that lacks a [#slug] marker.
 * - Any clause-numbered heading whose [#slug] marker violates the slug grammar.
 *
 * Warns on (during the v0.10 transition):
 * - Duplicate slug across any two headings. The later-seen occurrence
 *   (alphabetical path order) wins. Task 8 deletes the legacy chapter files,
 *   at which point duplicates self-resolve and the strict gate re-asserts.
 */
export function harvestClauses(contentRoot: string): Map<string, ClauseInfo> {
  const files: string[] = [];
  walkMdx(contentRoot, files);
  const map = harvestSluggedHeadings(files, contentRoot);
  assertAllNumberedHeadingsSlugged(files, contentRoot);
  return map;
}

export function defaultContentRoot(projectRoot: string): string {
  return path.join(projectRoot, 'src/content/docs');
}
