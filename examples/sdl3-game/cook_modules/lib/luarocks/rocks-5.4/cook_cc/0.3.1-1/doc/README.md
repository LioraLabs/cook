# cook-modules

Source-of-truth monorepo for Cook's blessed modules — published to
[`rocks.usecook.com`](https://rocks.usecook.com).

## Structure

One directory per module:

```
cook-modules/
├── cook_smoke/             # Phase 3 acceptance fixture (throwaway)
│   ├── cook_smoke.lua
│   ├── cook_smoke-<ver>.rockspec
│   └── README.md
├── cook_cpp/               # Phase 4 (planned)
├── cook_rust/              # Phase 4 (planned)
├── cook_pnpm/              # Phase 4 (planned)
└── cook_ai/                # Phase 4 (planned)
```

Each module is a self-contained Lua module installable via:

```
cook modules install cook_<name>
```

## Versioning + tagging

Module versions follow LuaRocks semantics: `<MAJOR.MINOR.PATCH>-<rockspec-revision>`.
Tags live at the repo root with the module name as a prefix:

```
cook_smoke-0.1.0-1
cook_cpp-1.2.4-1
```

Each tag pins a specific module's source. The rockspec's `source.url` is
`git+https://github.com/lioralabs/cook-modules.git` with `tag` and `dir`
fields scoping fetch to the module's subdirectory.

## Publishing a module

1. Author or update the module's source + rockspec in its subdir.
2. Bump the rockspec version.
3. Commit, then `git tag <module>-<ver>` and push tag.
4. `cook pack <module>` (Cookfile chore) — runs `luarocks pack` against the live tag.
5. `cook publish-to-index <module>` — copies the rockspec + .src.rock to
   `~/dev/cook-rocks/`, regenerates the manifest, and commits there.
6. From `~/dev/cook-rocks/`: `cook publish` — pushes to Gitea + GitHub mirror;
   Cloudflare Pages redeploys `rocks.usecook.com` within minutes.

CI for steps 4–6 on tag push is later work; v1 is manual.

## Repo topology

- **GitHub (`LioraLabs/cook-modules`)** — canonical. The rockspec source.url
  references this directly.
- **Gitea mirror (optional, future)** — for full-history redundancy on the
  NAS, mirroring the same pattern as `cook-rocks-index`.

## See also

- [`liora-labs/cook-rocks-index`](https://github.com/lioralabs/cook-rocks-index)
  — the rendered static index served as `rocks.usecook.com`.
- [`liora-labs/cook`](https://github.com/lioralabs/cook) — the cook engine
  that consumes these modules.
