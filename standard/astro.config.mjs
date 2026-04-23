import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  integrations: [
    starlight({
      title: 'The Cook Standard',
      description: 'The authoritative specification of the Cookfile language.',
      sidebar: [
        { label: 'Overview', link: '/' },
        {
          label: 'Normative',
          items: [
            { label: '§ 0 — Introduction',        link: '/00-introduction/' },
            { label: '§ 1 — Notation',            link: '/01-notation/' },
            { label: '§ 2 — Lexical structure',   link: '/02-lexical/' },
            { label: '§ 3 — Syntactic grammar',   link: '/03-syntactic-grammar/' },
            { label: '§ 4 — Recipes',             link: '/04-recipes/' },
            { label: '§ 5 — Execution model',     link: '/05-execution-model/' },
            { label: '§ 6 — Cook Lua API',        link: '/06-cook-lua-api/' },
            { label: '§ 7 — Modules',             link: '/07-modules/' },
          ],
        },
        {
          label: 'Appendices',
          collapsed: true,
          items: [
            { label: 'Appendix A — Grammar',        link: '/appendix/a-grammar/' },
            { label: 'Appendix B — Rationale',      link: '/appendix/b-rationale/' },
            { label: 'Appendix C — Worked examples', link: '/appendix/c-examples/' },
            { label: 'Appendix D — Changes',        link: '/appendix/d-changes/' },
          ],
        },
      ],
    }),
  ],
});
