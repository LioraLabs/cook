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
  would silently miss the index and render every node as never-cached
  (`src/dag_data.rs:339`).

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

## The structure it renders is a second implementation

These are not the engine's edges. `cook_engine::dag_builder` derives the real
work-unit DAG from the same `RecipeUnits`; `src/dag_data.rs` derives a parallel
one for display, re-deciding the sequential barrier, step-group entry and exit,
probe non-participation, `dep_edges`, and cross-recipe wiring. The comments
name their twin ("mirrors dag_builder.rs"), which satisfies one of the three
conditions `cook-contracts` places on a deliberate copy. There is no agreement
test, and the copy has drifted:

- **A withdrawn rule, still implemented.** `src/dag_data.rs:227` suppresses the
  whole-recipe `Barrier` edge when the consumer's `dep_edges` name the same
  upstream. That is CS-0161's fine-covered narrowing rule, which the Standard
  rejected: the shipped design is strictly additive, and a recipe that declares
  `requires` keeps byte-identical whole-recipe ordering whether or not its units
  carry fine refs (`standard/conformance/positive/dep-order-register-phase/notes.md:32`,
  pinned by `cook-engine/tests/dep_order.rs:177`). So `cook why` hides a barrier
  the engine does impose, which is the exact failure the edge-kind ordering
  above exists to prevent. It cannot be fixed where it stands: `analyzer.rs:67`
  merges `requires` and `orders` into one edge map before this crate sees it, so
  the distinction the branch needs is already gone. No test covers the branch.
- **No leaf pass-through.** The engine forwards a recipe's own deps' leaves
  when its barrier ends up empty, so a dependency routed through a unit-less
  meta-target still reaches the real upstream work. This crate records no
  terminals for such a recipe (`src/dag_data.rs:652`) and both consumers step
  past the missing entry (`src/dag_data.rs:197`, `src/dag_data.rs:239`), so the
  edge silently disappears from the picture.

The fix is not a third derivation; it is for `dag_builder` to hand over the
edges it already computes.

## Residue

- `ViewerError` (`src/lib.rs:36`) is never constructed. It outlived the
  ratatui viewer.
- `EdgeKind::is_ordering` (`src/dag_data.rs:122`) has no caller in this crate
  or any other.
- `UnitFacts::observed_builds_ago` and `Node::observed_max_age` describe an
  observation's age in retained builds. CS-0189 deleted that model:
  observations live in the step index now and may be served to a machine with
  no history of its own, where "three builds ago" names nothing. The only
  production caller passes `0` (`cook-cli/src/pipeline.rs:2090`), so
  `observed_max_age` is a permanent zero in the JSON payload and the
  ", up to N builds ago" rendering (`src/emit.rs:398`) is unreachable.
  `recorded_at` replaced it on every other surface.
- The whole `cook-engine` dependency is two calls: `render_ms`, a shim over a
  `cook-contracts` function this crate already depends on, and
  `recipe_cache_index_name`, which is pure, is documented as shared law
  ("`pub` because `cook-graph` performs the same lookup"), and by the admission
  bar belongs in `cook-contracts`. Move both and this crate stops depending on
  the engine.
