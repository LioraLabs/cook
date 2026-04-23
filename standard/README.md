# The Cook Standard

This project contains the authoritative specification of the Cookfile language and the static-site renderer that builds a modern reader for it.

## Contents

- `src/content/docs/` — the Standard's chapters and appendices (`.mdx`).
- `conformance/` — the conformance corpus consumed by `cook-lang`'s test suite and (planned) by `tree-sitter-cook`'s harness.
- `src/plugins/` — the remark/rehype plugins that drive spec-specific rendering (Cookfile syntax highlighting via `tree-sitter-cook`, RFC-2119 keyword styling, clause anchors, `CS-NNNN` permalinks).
- `src/styles/spec.css` — styling for the above.
- `scripts/check-normative-keywords.sh` — lint that flags lowercase RFC-2119 keywords in normative chapters.

## Building the site

```bash
pnpm install
pnpm build
```

The `prebuild` step compiles `../tree-sitter-cook` to WebAssembly (`public/tree-sitter-cook.wasm`). It requires either Docker or emscripten. The WASM artifact is gitignored; it is regenerated on each build.

## Development

```bash
pnpm dev                 # start Astro dev server
pnpm test                # run plugin tests
pnpm lint:keywords       # normative-keyword lint
```

## Hosting on the tailnet

```bash
pnpm build   # if dist/ isn't current
pnpm host
```

`pnpm host` runs `astro preview --host`, binding the preview server to `0.0.0.0:4321` so it's reachable from any tailnet device at `http://archbtw:4321/` (or `http://archbtw.story-pike.ts.net:4321/`). The tailnet hostname is allowlisted in `astro.config.mjs` under `vite.preview.allowedHosts`. If this repo ever moves to a different host, update that list.

## Changing the Standard

See `../CONTRIBUTING.md` for the spec-first rule. A change to a Cookfile surface construct must update `src/content/docs/` in the same commit as the implementation change, and must add a `CS-NNNN` entry to `src/content/docs/appendix/D-changes.mdx`.

Rendering-infrastructure changes (files under `src/plugins/`, `src/styles/`, `astro.config.mjs`, `package.json`, `tsconfig.json`) are not spec changes and do not require a `CS-NNNN` entry.
