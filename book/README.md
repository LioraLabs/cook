# The Cook Book

A friendly, tutorial-shaped guide to the Cookfile language and the `cook` CLI.

The Cook Book is the *learn-by-doing* counterpart to [The Cook Standard](../standard/). Where the Standard is the authoritative reference, the Book walks a reader from "I just installed cook" to "I can write modules and ship a real build pipeline" using small, runnable Cookfiles.

## Local development

```bash
pnpm install
pnpm dev
```

The dev server runs on http://localhost:4321 by default. The `predev` / `prebuild` scripts compile the tree-sitter-cook grammar to wasm so Cookfile snippets render with syntax highlighting that matches the Standard.

## Where this lives

The Book lives inside the cook monorepo at `book/`, alongside `standard/`. Both are content trees; a future `docs/` aggregator will combine them under a single Astro/Starlight site for `docs.usecook.com`, with route prefixes `/standard/*` and `/book/*`.

The Book's content is intentionally informal: Notes and Tips are conversational, examples are runnable end-to-end, and every claim about language behaviour links back to the corresponding clause in the Standard.

## Contributing

This is a rough first draft. Anything that disagrees with the Standard is a bug in the Book, not the Standard — file it the same way you'd file any other content issue.
