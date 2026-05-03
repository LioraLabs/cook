# Monorepo Codegen Example

A toy "schema → codegen → app config" pipeline that demonstrates Cook's
cross-Cookfile caching with `//`-anchored sigil imports between cousin
projects. Zero toolchain dependencies — everything is `cat` and `sed`.

## Layout

    examples/monorepo_codegen/
    ├── .cookroot                 # workspace anchor
    ├── Cookfile                  # top → cli + server (tree imports)
    ├── libs/
    │   ├── proto/Cookfile        # proto_lib
    │   └── queue/Cookfile        # imports //libs/proto
    └── apps/
        ├── cli/Cookfile          # imports //libs/proto
        └── server/Cookfile       # imports //libs/queue (transitive proto)

## The reference graph

                            ┌───────────┐
                            │   root    │
                            └─────┬─────┘
                tree ./apps/cli   │   tree ./apps/server
                   ┌──────────────┴──────────────┐
                   ▼                             ▼
             ┌───────────┐                 ┌───────────┐
             │ apps/cli  │                 │apps/server│
             └─────┬─────┘                 └─────┬─────┘
                   │ //libs/proto                │ //libs/queue
                   ▼                             ▼
             ┌───────────┐                 ┌───────────┐
             │libs/proto │◀────────────────│libs/queue │
             └───────────┘  //libs/proto   └───────────┘

`libs/proto` is reached two ways: directly from `apps/cli`, and
transitively from `apps/server` via `libs/queue`. Cook's diamond
dedup (Cook Standard §7.5) folds the two paths into a single build
of `proto_lib`.

## What gets built

Every recipe body is one or two `cat`/`sed` lines. Each Cookfile
emits one artifact:

| Cookfile         | Recipe       | Output             |
|------------------|--------------|--------------------|
| `libs/proto/`    | `proto_lib`  | `build/proto.bin`  |
| `libs/queue/`    | `queue_lib`  | `build/queue.bin`  |
| `apps/cli/`      | `cli_bin`    | `build/cli.bin`    |
| `apps/server/`   | `server_bin` | `build/server.bin` |
| (root) `Cookfile`| `top`        | `build/release.bin`|

The cross-Cookfile body refs (`{proto.proto_lib}`, `{queue.queue_lib}`,
`{cli.cli_bin}`, `{server.server_bin}`) lower to
`cook.dep_output("…")` calls during workspace codegen, with paths
rewritten relative to the importing Cookfile.

## Walkthrough

Run `bash walkthrough.sh` to see all five scenarios at once with
PASS/FAIL assertions. Or play along by hand:

### Scenario 1 — Fresh build

    $ cook top
    cook build done (… 0 cached recipes, 5 done)

All five recipes run. The `.cookroot` marker anchors the workspace;
the import graph resolves in one pass.

### Scenario 2 — No-op rebuild

    $ cook top
    cook build done (… 5 cached recipes, 5 done)

Everything content-addresses to a hit. Note: the cache extends across
Cookfile boundaries — `apps/cli`'s cache key includes the hash of
`libs/proto/build/proto.bin`, even though that file lives in a sibling
package.

### Scenario 3 — Schema drift

Edit `libs/proto/schema.proto` (add a new message, change a field name,
anything). Rebuild:

    $ cook top
    cook build done (… 0 cached recipes, 5 done)

Everything invalidates. `proto_lib`'s output hash changes, which
cascades through `queue_lib`, `cli_bin`, `server_bin`, and `top`. The
diamond is the whole graph here — proto is upstream of every node.

### Scenario 4 — Queue-only drift

Restore `schema.proto`, run a clean fresh build, then edit
`libs/queue/queue.tmpl`:

    $ cook top
    cook build done (… 2 cached recipes, 5 done)

Two cached: `proto_lib` (its inputs didn't change) and `cli_bin`
(it imports proto, not queue). Three rebuilt: `queue_lib`,
`server_bin`, `top`. **This is the case sigil imports earn their
keep on** — cousins that don't share a transitive dep stay
independent.

### Scenario 5 — Subdir invocation

`cd` into a deep subdirectory and build a single recipe:

    $ cd libs/queue
    $ cook queue_lib
    cook build done (… 0 cached recipes, 2 done)

Cook walks up looking for `.cookroot` (Cook Standard §7.6 Rule 2),
finds it at the workspace root, and resolves `//libs/proto` from
there. Two nodes: `proto_lib` (transitive dep) and `queue_lib`.

## How it works

Cook resolves `//`-anchored imports against the workspace root, found
via `.cookroot` walk-up. Cross-Cookfile body refs (`{alias.recipe}`)
lower to dep-output references whose paths are rewritten relative to
the importer. The cache is content-addressed and includes input file
hashes, so changes propagate exactly along the dep graph — no more, no
less. See Cook Standard §7.2, §7.5, §7.6, and §7.7 for the formal
semantics.

## Cleanup

    $ cook clean
