# cook-graph

`cook-graph` renders a target's dependency structure at a size a person can
read. It is the graph half of `cook why`: it builds the unit-level graph,
collapses it to a chosen granularity, and returns it as text, mermaid, dot, or
JSON.

## How it does that well

- **It collapses before it renders.** DuckDB registers 1711 units and no
  layout makes that readable, so recipe level is the default and `--level unit`
  refuses past `UNIT_LEVEL_SOFT_CAP` (`src/emit.rs:55`) rather than emitting
  something nobody can read. The flat per-unit report `cook why` printed before
  CS-0171 had no ceiling and no aggregation.
- **A collapsed node keeps counts, never a boolean.** A recipe with 339 hits
  and one rebuild is a different finding from one with 340 rebuilds; the
  `[cached]`/`[stale]` marker this replaced rendered the two identically
  (§17.1.6.2).
- **A collapsed edge keeps the most constraining kind in its bundle.**
  `EdgeKind` is ordered weakest-to-strongest for exactly that
  (`src/dag_data.rs:84`), so a whole-recipe barrier cannot vanish behind the
  data edges it travels with. "Why is my build serial" is half of what this
  command exists to answer.
- **It computes no cache verdict.** It used to: a private local-index-only
  rebuild check that knew nothing of the shared tier and could not re-fold a
  sealed unit's probe values, so it gave a strictly weaker answer wearing the
  same clothes. CS-0171 deleted it. A node now carries only `(recipe,
  cache_key)`, the join key `cook_engine::why` and `cook_engine::observations`
  attach their findings to.
- **Absence never renders as zero.** A unit with no cache verdict counts as
  `unclassified` rather than as a rebuild; a unit never timed counts as
  `unobserved` rather than as 0ms; a file node counts toward nothing at all,
  not even `units`, because giving it a span of zero units taking zero time is
  the same lie in another shape (`src/emit.rs:215`, §17.1.6.4).
- **`forces` counts every downstream unit, not just the ones already
  classified as rebuilding.** At the moment the query runs a downstream unit is
  usually still a hit: its inputs have not changed *yet*, because the upstream
  rebuild has not happened. Counting only current rebuilds would report zero in
  precisely the case the number exists for. The rendering says "invalidates"
  rather than "will rebuild", because a rebuild that reproduces byte-identical
  output leaves its consumers cached.
- **It has no waves.** The engine stopped scheduling by wave at SHI-222 Phase
  4. The wave grouping that lingered here manufactured whole-recipe edges the
  engine does not impose: 1685 synthetic edges standing in for one real one on
  DuckDB, enough to mask the genuine per-unit edges under any aggregation. The
  ratatui browser that navigated by wave went with them.
- **The cascade walk runs over the collapsed graph, not the unit graph**
  (`src/emit.rs:310`), which is what makes a per-node reachability DFS
  affordable when the graph it came from holds thousands of nodes.
- **It borrows the cache index naming rule rather than guessing.** The on-disk
  index is written under the units' Cookfile-local `CacheMeta.recipe_name`, not
  the import-qualified workspace key; loading `rust.build` instead of `build`
  would silently miss the index and render every node as never-cached. The rule
  is `cook_contracts::cache::recipe_cache_index_name` (`src/dag_data.rs:332`),
  the same law the executor writes under and `cook cache verify` reads by.

## What it does not do

It does not decide whether a unit will run: `cook_engine::why` owns the tier
verdict and the determinant values, `cook_engine::observations` owns what
anything cost, and `Annotations` is the seam those arrive through. It does not
schedule, execute, or order anything; nothing here sits on a build's critical
path. It does not print: `render` returns a `String` and `json_value` returns a
`serde_json::Value`, so the caller can graft the determinant half onto the same
document (§17.1.6.5). It formats no durations of its own; that law lives in
`cook_contracts::render::duration_ms` (COOK-392). It owns no generic graph
algorithms.

## Relationship to `cook-dag`

Two crate names that both mean "the dependency structure" invite the question,
and the distinction is real. `cook-dag` is a dependency-free generic `Dag<T>`
with topological traversal and an atomic ready-count: it is the structure the
executor actually runs. `cook-graph` builds no `Dag<T>` at all; it produces a
flat `Vec<NodeData>` / `Vec<EdgeData>` that exists to be looked at. One is the
machine's graph, the other is the reader's. The names are the wrong way round
for that split, but only the names.

## The structure it renders is the engine's

These are the engine's edges, by construction. The unit-to-unit wiring —
barriers, step groups, probe consumption and pruning, `dep_edges`, coarse
cross-recipe barriers, leaf pass-through — is
`cook_contracts::unit_graph::plan` (COOK-402 / CS-0202), the same pure plan
`cook_engine::dag_builder` lowers into the DAG the executor runs. This crate
maps the plan's per-dependency provenance onto `EdgeKind` and adds only the
display layer the plan does not carry: file nodes, declared/discovered input
edges, producer→consumer data edges, labels, staleness.

It was not always so. `src/dag_data.rs` used to re-derive the wiring in
parallel — a deliberate copy with no agreement test — and it drifted twice:
it kept implementing CS-0161's fine-covered narrowing rule after the Standard
withdrew it (hiding a declared barrier the engine imposes, the exact failure
the edge-kind ordering above exists to prevent), and it recorded no terminals
for an empty-barrier recipe where the engine forwards its deps' leaves, so a
dependency routed through a unit-less meta-target vanished from the output.
The suppression was also unfixable in place: the closure edge map merges
`requires` and `orders` before this crate sees it, so the branch could not
distinguish the case it wanted. One decision, one home, and both bugs became
unrepresentable.

## Residue

- `ViewerError` (`src/lib.rs:36`) is never constructed. It outlived the
  ratatui viewer.
- `UnitFacts::observed_builds_ago` and `Node::observed_max_age` describe an
  observation's age in retained builds. CS-0189 deleted that model:
  observations live in the step index now and may be served to a machine with
  no history of its own, where "three builds ago" names nothing. The only
  production caller passes `0` (`cook-cli/src/pipeline.rs:2090`), so
  `observed_max_age` is a permanent zero in the JSON payload and the
  ", up to N builds ago" rendering (`src/emit.rs:398`) is unreachable.
  `recorded_at` replaced it on every other surface.
