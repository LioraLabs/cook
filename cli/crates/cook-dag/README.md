# cook-dag

`cook-dag` tracks readiness. Given a fixed graph of dependent work it answers
which nodes may start now, and as each node finishes it names the ones that
just became startable.

## How it does that well

- `complete(id)` returns exactly the nodes that *transitioned* to ready, not
  the current ready set. Nobody polls, and a transition is observed once, by
  the thread that caused it. The executor's eight completion sites each feed
  that return value straight into the work queue, so "who runs next" has one
  answer and one producer of it.
- It takes `&self`, not `&mut self`. Readiness is a per-node `AtomicUsize`, so
  worker threads finishing different nodes release their dependents
  concurrently with no lock around the graph. `Dag<T>` and `Node<T>` are held
  to `Send + Sync` by a test, not by a comment.
- Ids are indices assigned in insertion order, and `add_node` rejects any
  dependency whose id is not strictly smaller. That one rule makes id order a
  topological order: the graph is acyclic *by construction*, which is a
  stronger guarantee than checking for cycles afterwards and cheaper than
  either.
- `add_node` de-duplicates `depends_on` through a `BTreeSet` before it wires
  anything, so the dependency count and the forward edges cannot disagree. The
  failure mode this forecloses is silent and total: a node whose count says 2
  behind a single edge never becomes ready, and the build hangs with no error
  to print. The engine's builder synthesises dependencies from several sources
  at once (probe producers, cross-recipe leaves, within-recipe ordering) and is
  in a good position to hand over the same id twice (cbc8278a).
- `add_node` validates before it mutates and returns `Result`. It used to panic
  on an out-of-range dependency; now a rejected insert leaves the graph exactly
  as it was, so the caller sees a `DagError` and an unchanged DAG rather than a
  half-wired one (cbc8278a).
- `Node`'s fields are `pub(crate)` behind accessors. The two invariants that
  make everything above true (id equals index, `remaining_deps` equals the
  inbound edge count) survive only if no caller can write a `Node` literal;
  the same commit that hardened `add_node` closed that door (cbc8278a).
- `CycleError` carries a concrete path rather than a count. The diagnostic
  prints `v0 -> v1 -> v0` plus how many nodes are blocked; the previous bare
  count told a user that something was circular but not what (cbc8278a).
- It has no dependencies at all, not even `cook-contracts`. `Dag<T>` is a data
  structure, not a shared law: no other crate has to agree with it about
  anything, because there is exactly one implementation and `T` is opaque to
  it. Nothing here meets the `cook-contracts` admission bar and nothing here
  wants to.

## A standing finding: `validate()` cannot fail

`add_node` is the only mutator, and it refuses forward references. Every edge
therefore points from a higher id to a lower one, which means no DAG built
through this crate's public API can contain a cycle, which means `validate()`
returns `Ok` for every input a caller can construct.

Kahn's pass, `extract_cycle`, `CycleError`, and its `Display` are roughly a
third of `lib.rs`, and they are unreachable from outside. The two tests that
exercise them have to reach into the crate-private `deps` and `nodes` vectors
to forge a cycle, which is the tell: a test that must violate the type's
invariant to reach the code under test is testing something the type cannot
do. `cook-engine`'s executor calls `validate()` before spawning workers and
its own comment concedes the check is defensive.

This is recorded rather than removed because the cost is one O(V+E) pass per
run and the diagnostic is genuinely good if a future mutator ever relaxes the
ordering rule. But it should be read as dead weight with a rationale, not as a
feature, and if the ordering rule is still standing at the next audit the
honest move is to delete the cycle machinery and let `add_node`'s range check
be the whole story.

## What it does not do

It does not know what the work is. `T` is never inspected; `WorkNode` and its
recipe names, cache metadata, and chore windows are `cook-engine`'s business
entirely.

It does not schedule. It says which nodes are eligible; the choice of which
eligible node to run, on which thread, with what concurrency limit, and the
chore-window lookahead that peeks along `dependents()` all live in
`cook-engine::executor`.

It does not detect the cycles users actually write. A recipe that requires
itself is caught upstream by `cook-engine::analyzer`'s DFS, and an import loop
at workspace load time; both report before a `Dag` is ever built. See the
section above.

It does not defend against double completion. `complete` is `fetch_sub` with
no once-only guard, so completing a node twice wraps a dependent's counter and
can release a node whose other predecessors have not run. That invariant is
currently owned by all eight of the executor's call sites rather than by this
crate.

It offers no removal, no re-parenting, no edge insertion after the fact, and
no re-arming: a `Dag` is built once, drained once, and dropped. `initial_ready`
reads live counters, so calling it mid-drain returns everything currently
unblocked rather than the roots.

## Relationship to `cook-graph`

Two crate names that both mean "the dependency structure" is a fair thing to
be suspicious of. The distinction is real and neither crate depends on the
other:

- `cook-dag` is the **runtime** structure: mutable atomic counters, generic
  payload, zero dependencies, consumed only by the executor while a build is
  in flight. It exists to make a scheduling decision.
- `cook-graph` is the **reported** structure: a serializable `DagData` of
  `NodeData`/`EdgeData` with a wire-format schema version, built after the
  fact from `RecipeUnits` and cache managers, rendered by `cook why`. It
  exists to explain a build to a human, and it deliberately computes no
  verdicts of its own (CS-0171).

One is the graph the machine walks; the other is the graph the user reads.
`cook why`'s payload is not built by traversing a `Dag<T>`, and the executor
never constructs a `DagData`. Note also that `cook dag` as a CLI subcommand
was deleted and folded into `cook why` at CS-0171, so the name no longer
collides at the surface, only in `crates/`.
