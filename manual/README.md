# docs.usecook.com

The Cook Manual as a site, in the ~/notes format (markdown, wikilinks, callouts, mermaid, slides, search).

- `content/manual/` is **generated** from `../document.md` on every dev and build. Edit the manual, not the output.
- `content/guides/` is hand-written MDX: terminal replays (`Terminal.astro`), the pipeline motion graphic, a slide deck.
- Code fences tagged `cook` highlight with the grammar in `src/grammar/`; the manual's untagged Cookfile fences are tagged by the splitter.

```sh
npm run dev      # http://127.0.0.1:4400
npm run build && npm test
npm run deploy   # Worker "cook-docs"; uncomment the route in wrangler.jsonc to attach docs.usecook.com
```
