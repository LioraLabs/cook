# Monorepo Example — pnpm cook_module

Demonstrates Cook orchestrating a pnpm workspace with correct
`catalog:` specifier resolution.

## Prerequisites

- Node.js (>= 18)
- pnpm (>= 9.5, for catalog support)
- Cook

## Setup

```bash
cd examples/monorepo
pnpm install
```

## Usage

```bash
cook build    # builds all packages in dependency order
cook test     # runs tests (depends on build)
cook clean    # removes dist/ and .cook/ artifacts
```

## What this demonstrates

This workspace has three packages with two types of internal dependency
specifiers:

- **shared-utils**: leaf package, no workspace dependencies
- **ui**: depends on `shared-utils` via `catalog:internal` — a pnpm catalog
  specifier that turborepo failed to resolve (vercel/turborepo#10785)
- **web**: depends on `ui` via `workspace:*` — standard pnpm workspace protocol

The `pnpm` cook_module reads `pnpm-workspace.yaml` (including `catalog:` and
`catalogs:` entries), resolves all specifiers to workspace edges, topologically
sorts packages, and emits one `cook.add_unit()` per (package, task) tuple.

Cook's native caching means subsequent runs skip packages whose inputs
haven't changed.

## The catalog resolution bug

pnpm 9.5 introduced catalogs — centralized dependency versions in
`pnpm-workspace.yaml`. When a `catalog:name` specifier resolved to a workspace
package, turborepo's graph builder didn't recognize it as an internal dependency,
causing missing edges in the task DAG. This module resolves the full
`catalog:name` → catalog lookup → `workspace:*` chain correctly.
