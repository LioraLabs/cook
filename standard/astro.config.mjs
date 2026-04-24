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

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const knownCsIds = harvestCsIds(defaultChangesPath(__dirname));
const clauseMap = harvestClauses(defaultContentRoot(__dirname));

export default defineConfig({
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
  vite: {
    preview: {
      allowedHosts: ['archbtw', 'archbtw.story-pike.ts.net'],
    },
  },
});
