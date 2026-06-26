# The Cook Standard

This project contains the authoritative specification of the Cookfile language and the static-site renderer that builds a modern reader for it.

## Contents

- `src/content/docs/` — the Standard's chapters and appendices (`.mdx`).
- `conformance/` — the conformance corpus consumed by `cook-lang`'s test suite and (planned) by `tree-sitter-cook`'s harness.
- `src/plugins/` — the remark/rehype plugins that drive spec-specific rendering (Cookfile syntax highlighting via `tree-sitter-cook`, RFC-2119 keyword styling, clause anchors, `CS-NNNN` permalinks).
- `src/styles/spec.css` — styling for the above.
- `cook_modules/checks.lua` — repo-local checks: the normative-keyword lint and the backwards-conformance harness, exposed as `cook standard.lint` and `cook standard.against-tag`.

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
cook standard.lint       # normative-keyword lint (routes through cook_modules/checks.lua)
```

## Hosting the preview on your network

```bash
pnpm build   # if dist/ isn't current
pnpm host
```

`pnpm host` runs `astro preview --host`, binding the preview server to `0.0.0.0:4321` so it's reachable from other devices on your network (e.g. `http://<your-host>:4321/`). If your environment requires it, allowlist the host in `astro.config.mjs` under `vite.preview.allowedHosts`.

## Changing the Standard

See `../CONTRIBUTING.md` for the spec-first rule. A change to a Cookfile surface construct must update `src/content/docs/` in the same commit as the implementation change, and must add a `CS-NNNN` entry to `src/content/docs/appendix/D-changes.mdx`. To publish a new MINOR version of the Standard, see the **Cutting a Cook Standard version** subsection in the same file.

Rendering-infrastructure changes (files under `src/plugins/`, `src/styles/`, `astro.config.mjs`, `package.json`, `tsconfig.json`) are not spec changes and do not require a `CS-NNNN` entry.
