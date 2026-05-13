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
          label: 'Normative',
          items: [
            { label: '§ 0 — Introduction',        link: '/00-introduction/' },
            { label: '§ 1 — Notation',            link: '/01-notation/' },
            { label: '§ 2 — Lexical structure',   link: '/02-lexical/' },
            { label: '§ 3 — Syntactic grammar',   link: '/03-syntactic-grammar/' },
            { label: '§ 4 — Recipes',                     link: '/04-recipes/' },
            { label: '§ 5 — Cross-recipe references',     link: '/05-cross-recipe-references/' },
            { label: '§ 6 — Cook Lua API',                link: '/06-cook-lua-api/' },
            { label: '§ 7 — Cross-Cookfile composition',  link: '/07-cross-cookfile-composition/' },
            { label: '§ 8 — Execution model',             link: '/08-execution-model/' },
          ],
        },
        {
          label: 'Appendices',
          collapsed: true,
          items: [
            { label: 'Appendix A — Grammar',         link: '/appendix/a-grammar/' },
            { label: 'Appendix B — Rationale',       link: '/appendix/b-rationale/' },
            { label: 'Appendix C — Worked examples', link: '/appendix/c-examples/' },
            { label: 'Appendix D — Changes',         link: '/appendix/d-changes/' },
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
        changesHref: '/appendix/d-changes/',
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
