import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { remarkCookHighlight } from './src/plugins/remark-cook-highlight.ts';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  integrations: [
    starlight({
      title: 'The Cook Book',
      description: 'A friendly introduction to the Cookfile language and the cook CLI.',
      customCss: ['./src/styles/book.css'],
      sidebar: [
        { label: 'Welcome', link: '/' },
        {
          label: 'Getting started',
          items: [
            { label: '1. Install cook',           link: '/01-install/' },
            { label: '2. Your first recipe',      link: '/02-hello/' },
            { label: '3. Recipes & dependencies', link: '/03-recipes-and-deps/' },
          ],
        },
        {
          label: 'Building things',
          items: [
            { label: '4. Ingredients & cook',     link: '/04-ingredients-and-cook/' },
            { label: '5. plate & test',           link: '/05-plate-and-test/' },
            { label: '6. Chores',                 link: '/06-chores/' },
            { label: '7. Substitutions & paths',  link: '/07-substitutions/' },
          ],
        },
        {
          label: 'Going further',
          items: [
            { label: '8. Lua: registering work',  link: '/08-lua/' },
            { label: '9. Modules & composition',  link: '/09-modules/' },
            { label: '10. The cook CLI',          link: '/10-cli/' },
          ],
        },
        {
          label: 'Appendices',
          collapsed: true,
          items: [
            { label: 'A — Common patterns',  link: '/appendix/a-patterns/' },
            { label: 'B — Glossary',         link: '/appendix/b-glossary/' },
            { label: 'C — Where to next',    link: '/appendix/c-where-to-next/' },
          ],
        },
      ],
    }),
  ],
  markdown: {
    remarkPlugins: [
      [remarkCookHighlight, {
        wasmPath: path.join(__dirname, 'public/tree-sitter-cook.wasm'),
        queryPath: path.join(__dirname, '../tree-sitter-cook/queries/highlights.scm'),
      }],
    ],
  },
  vite: {
    preview: {
      allowedHosts: true,
    },
  },
});
