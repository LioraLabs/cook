import { defineConfig } from 'astro/config';
import { unified } from '@astrojs/markdown-remark';
import mdx from '@astrojs/mdx';
import expressiveCode, { ExpressiveCodeTheme } from 'astro-expressive-code';
import codeTheme from './code-theme.mjs';
import cookGrammar from './src/grammar/cook.tmLanguage.json' with { type: 'json' };
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import wikilinks from './wikilinks.mjs';
import slides from './slides.mjs';
import callouts from 'rehype-callouts';
import autolink from 'rehype-autolink-headings';
import rehypeSlug from 'rehype-slug';
import mermaid from 'astro-mermaid';

// docs.usecook.com: the notes engine (~/dev/notes) pointed at ./content. content/manual/ is generated from ../document.md.
export default defineConfig({
  site: 'https://docs.usecook.com',
  integrations: [
    mermaid({ theme: 'dark' }), // before expressiveCode, or it claims ```mermaid fences as code
    expressiveCode({
      themes: [new ExpressiveCodeTheme(codeTheme)],
      shiki: { langs: [cookGrammar] },
      styleOverrides: {
        borderRadius: '0', borderColor: 'var(--line)', codeBackground: 'var(--surface)',
        codeFontFamily: "'JetBrains Mono', ui-monospace, monospace", codeFontSize: '0.85rem',
        frames: { frameBoxShadowCssValue: 'none' }, uiFontFamily: "'JetBrains Mono', ui-monospace, monospace",
      },
    }),
    mdx(),
  ],
  markdown: { processor: unified({ remarkPlugins: [remarkMath, wikilinks], rehypePlugins: [rehypeKatex, [callouts, { theme: 'obsidian' }], rehypeSlug, [autolink, { behavior: 'append', properties: { className: ['anchor'], ariaHidden: true, tabIndex: -1 }, content: [] }], slides] }) },
});
