import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { remarkSlugXrefs } from './src/plugins/remark-slug-xrefs.ts';
import { remarkCookHighlight } from './src/plugins/remark-cook-highlight.ts';
import { remarkRfc2119 } from './src/plugins/remark-rfc2119.ts';
import { rehypeClauseAnchors } from './src/plugins/rehype-clause-anchors.ts';
import { rehypeClauseXrefs } from './src/plugins/rehype-clause-xrefs.ts';
import { rehypeCsPermalinks } from './src/plugins/rehype-cs-permalinks.ts';
import { rehypeBareRefLint } from './src/plugins/rehype-bare-ref-lint.ts';
import { harvestCsIds, defaultChangesPath } from './src/plugins/cs-ids.ts';
import { harvestClauses, defaultContentRoot } from './src/plugins/clauses.ts';
import { SLUG_RENAMES } from './scripts/slug-renames.ts';
import { SLUG_MAPPING } from './scripts/slug-mapping.ts';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const knownCsIds = harvestCsIds(defaultChangesPath(__dirname));
const clauseMap = harvestClauses(defaultContentRoot(__dirname));

// Page-level redirects for the v0.10 structural redesign. The Standard's
// chapter URLs change wholesale; these redirects keep external links alive.
// Anchor-level redirects (fragment rewrites driven by SLUG_RENAMES) are out
// of scope here — see the v0.10 reorg CS entry for the deferred follow-up.
//
// SLUG_RENAMES and SLUG_MAPPING are imported so that a future enhancement
// can compute the fragment-rewrite table from the same source of truth that
// the build-time plugins consult.
void SLUG_RENAMES;
void SLUG_MAPPING;

export default defineConfig({
  redirects: {
    '/03-syntactic-grammar/':            '/04-toplevel/',
    '/04-recipes/':                      '/06-recipes/',
    '/04a-chores/':                      '/07-chores/',
    '/05-cross-recipe-references/':      '/10-cross-recipe-references/',
    '/06-cook-lua-api/':                 '/21-lua-api/',
    '/07-cross-cookfile-composition/':   '/11-cross-cookfile-composition/',
    '/08-execution-model/':              '/13-two-phase/',
    '/09-standard-modules/':             '/27-catalogue/',
    '/01-notation/':                     '/02-notation/',
    '/02-lexical/':                      '/03-lexical/',
    '/appendix/b-rationale/':            '/appendix/c-rationale/',
    '/appendix/c-examples/':             '/appendix/b-examples/',
    '/appendix/d-changes/':              '/appendix/e-changes/',
    '/appendix/e-pre-v1-checklist/':     '/appendix/d-pre-v1-checklist/',
  },
  integrations: [
    starlight({
      title: 'The Cook Standard',
      description: 'The authoritative specification of the Cookfile language.',
      customCss: ['./src/styles/spec.css'],
      sidebar: [
        { label: 'Overview', link: '/' },
        {
          label: 'Front matter',
          items: [
            { label: '§ 0 — Introduction', link: '/00-introduction/' },
            { label: '§ 1 — Conformance',  link: '/01-conformance/' },
            { label: '§ 2 — Notation',     link: '/02-notation/' },
          ],
        },
        {
          label: 'Part I — The Cookfile language',
          items: [
            { label: '§ 3 — Lexical structure',          link: '/03-lexical/' },
            { label: '§ 4 — Top-level structure',        link: '/04-toplevel/' },
            { label: '§ 5 — Declarations',               link: '/05-declarations/' },
            { label: '§ 6 — Recipes',                    link: '/06-recipes/' },
            { label: '§ 7 — Chores',                     link: '/07-chores/' },
            { label: '§ 8 — Step kinds',                 link: '/08-step-kinds/' },
            { label: '§ 9 — Placeholders',               link: '/09-placeholders/' },
            { label: '§ 10 — Cross-recipe references',   link: '/10-cross-recipe-references/' },
            { label: '§ 11 — Cross-Cookfile composition',link: '/11-cross-cookfile-composition/' },
            { label: '§ 12 — Modules',                   link: '/12-modules/' },
          ],
        },
        {
          label: 'Part II — Execution model',
          items: [
            { label: '§ 13 — Two-phase model',           link: '/13-two-phase/' },
            { label: '§ 14 — Capture mode',              link: '/14-capture-mode/' },
            { label: '§ 15 — Step groups',               link: '/15-step-groups/' },
            { label: '§ 16 — Ordering & drain',          link: '/16-ordering-drain/' },
            { label: '§ 17 — Cache semantics',           link: '/17-cache/' },
            { label: '§ 18 — Output materialisation',    link: '/18-output-materialisation/' },
            { label: '§ 19 — Diagnostic ordering',       link: '/19-diagnostics/' },
            { label: '§ 20 — Workspace root',            link: '/20-workspace/' },
          ],
        },
        {
          label: 'Part III — The Cook Lua API',
          items: [
            { label: '§ 21 — API surface overview',      link: '/21-lua-api/' },
            { label: '§ 22 — Register-phase API',        link: '/22-register-phase/' },
            { label: '§ 23 — Execute-phase API',         link: '/23-execute-phase/' },
            { label: '§ 24 — Both-phase API',            link: '/24-both-phase/' },
            { label: '§ 25 — fs.* (incl. sandbox)',      link: '/25-fs/' },
            { label: '§ 26 — path.*',                    link: '/26-path/' },
          ],
        },
        {
          label: 'Part IV — Standard module catalogue',
          items: [
            { label: '§ 27 — Catalogue governance',      link: '/27-catalogue/' },
            { label: '§ 28 — cc — C-family build module',link: '/28-cc/' },
          ],
        },
        {
          label: 'Annexes',
          collapsed: true,
          items: [
            { label: 'Appendix A — Grammar',           link: '/appendix/a-grammar/' },
            { label: 'Appendix B — Worked examples',   link: '/appendix/b-examples/' },
            { label: 'Appendix C — Rationale',         link: '/appendix/c-rationale/' },
            { label: 'Appendix D — Pre-1.0 checklist', link: '/appendix/d-pre-v1-checklist/' },
            { label: 'Appendix E — Changes',           link: '/appendix/e-changes/' },
            { label: 'Appendix F — Conformance corpus',link: '/appendix/f-corpus/' },
          ],
        },
      ],
    }),
  ],
  markdown: {
    remarkPlugins: [
      remarkSlugXrefs,
      [remarkCookHighlight, {
        wasmPath: path.join(__dirname, 'public/tree-sitter-cook.wasm'),
        queryPath: path.join(__dirname, '../tree-sitter-cook/queries/highlights.scm'),
      }],
      remarkRfc2119,
    ],
    rehypePlugins: [
      rehypeClauseAnchors,
      [rehypeClauseXrefs, { clauseMap }],
      [rehypeCsPermalinks, {
        knownIds: knownCsIds,
        changesHref: '/appendix/e-changes/',
      }],
      rehypeBareRefLint,
    ],
  },
  server: {
    allowedHosts: true,
  },
  vite: {
    preview: {
      allowedHosts: true,
    },
  },
});
