# Monorepo Example — the `cook_pnpm` blessed module

Demonstrates Cook orchestrating a real pnpm workspace through the blessed
[`cook_pnpm`](https://rocks.usecook.com) module, with correct `catalog:`
and `workspace:*` specifier resolution and content-addressed caching keyed
on the pnpm lockfile.

This example is a genuine module-contract consumer: it pins `cook_pnpm` in
`cook.toml`/`cook.lock` and loads it from a LuaRocks-installed
`cook_modules/` tree — it does **not** vendor a hand-written copy.

## Prerequisites

- Node.js (>= 18) and pnpm (>= 9.5, for catalog support) on `PATH`
- Cook, with its bundled LuaRocks (`~/.cook/bin/luarocks`)

## Setup

```bash
cd examples/modules/monorepo

# 1. Install the JS toolchain + workspace links (generates pnpm-lock.yaml).
pnpm install

# 2. Realise the pinned cook_pnpm into ./cook_modules.
#    Published consumers run `cook install`, which reads cook.toml/cook.lock
#    and fetches from rocks.usecook.com. To consume an UNPUBLISHED build of
#    cook_pnpm (e.g. a local module checkout) install it directly with the
#    bundled 5.4 LuaRocks:
~/.cook/bin/luarocks make --tree cook_modules \
    /path/to/cook-modules/cook_pnpm/cook_pnpm-0.2.0-1.rockspec
```

The install lands `cook_modules/share/lua/5.4/cook_pnpm/…`, the LuaRocks
search path Cook adds for every recipe (Cook Standard §12.2). `cook_modules/`
is git-ignored — only the `cook.toml` pin and `cook.lock` are committed.

## Usage

```bash
cook build    # builds every package in dependency order (tsc per package)
cook check    # runs each package's test (depends on build)
cook clean    # removes each package's dist/ and the .cook cache dirs
```

## What this demonstrates

Three packages with two kinds of internal-dependency specifier:

- **shared-utils** — leaf package, no workspace dependencies
- **ui** — depends on `shared-utils` via `catalog:internal` (a pnpm catalog
  specifier)
- **web** — depends on `ui` via `workspace:*` (the workspace protocol)

`cook_pnpm.workspace()` parses `pnpm-workspace.yaml` and each package's
`package.json`, resolves every dependency whose name is a workspace member
into a graph edge, and topologically sorts the packages.
`cook_pnpm.task("build", …)` then emits one `<pkg>:build` recipe per
package, wired in topo order.

### Caching and the lockfile seal

Each emitted unit folds two probes into its cache key by different
dispositions (Cook Standard §12.7.5, the module-authoring seal policy):

- the **toolchain probe** (`pnpm:toolchain:<pin>`) is consumed as *data* —
  the command interpolates the resolved `pnpm` path — so its value folds in
  as a `probes` entry;
- the **install probe** (`pnpm:install:<lockfile-hash>`) is a deterministic,
  *invalidate-only* determinant, so it is carried as a **`seal`**.

The observable effect: editing `pnpm-workspace.yaml` / a `package.json`
dependency and re-running `pnpm install` rewrites `pnpm-lock.yaml`, which
changes the `pnpm:install:<hash>` key and invalidates every sealed build
unit; a source-only rebuild with an unchanged lockfile is a full cache hit.
