# cook by example

Ten small, runnable projects, in learning order. Each one is a working
build you can `cd` into and run — every README has a "watch the cache
work" moment, because the cache **is** the product: cook keys every unit
of work on the content of its inputs, its command, and its declared
environment, and never does the same work twice.

| # | example | you learn |
|---|---------|-----------|
| 01 | [hello-cook](01-hello-cook/) | a recipe, ingredients, one fan-out `cook` step; the second run is free |
| 02 | [pipeline](02-pipeline/) | multi-stage: fan-out → many-to-one (`$<all>`) → multi-output; edits rebuild exactly what they invalidate |
| 03 | [chores-and-config](03-chores-and-config/) | uncached parameterized chores; config blocks, `@preset` overlays, `--set`; env vars as cache keys |
| 04 | [probes](04-probes/) | computed values the cache can see: shell/lines/json/Lua producers, probe chains, fan-out over members |
| 05 | [data-fanout](05-data-fanout/) | the build's shape from a JSON manifest; grow the data, build only the new member; per-member joins |
| 06 | [lua-recipes](06-lua-recipes/) | execute-time Lua bodies, computed output paths, the register-time low-level API |
| 07 | [testing](07-testing/) | tests as cached steps; `as`/`timeout`/`should_fail`; the `cook test` runner |
| 08 | [workspace](08-workspace/) | many Cookfiles, one workspace: imports, `//` targets, run cook from any subdirectory |
| 09 | [deploy](09-deploy/) | `plate` — the unsandboxed ship-it step; artifacts cache, side effects don't pretend to |
| 10 | [cache-trust](10-cache-trust/) | who may write your cache: `seal`, `local`, `pinned`, `nondet`; `cook why`; cross-machine sharing |

Examples that require installed modules (C/C++ via `cook_cc`, pnpm
workspaces) live in [modules/](modules/).

Useful everywhere: `cook menu` (what can I run?), `cook why <recipe>`
(why did/didn't this rebuild?), `cook emit-lua` (what does this Cookfile
lower to?).
