# cook-contracts

`cook-contracts` defines Cook's shared language as plain data types and pure
rules.

It owns the contracts exchanged between Cook crates: captured work, recipes,
cache metadata, probes, registration names, and canonical value rendering.
Each concept lives in its own directory module, while the crate root remains an
index that preserves the established public imports.

It does not own filesystem or environment access, process execution, VM policy,
cache backends, scheduling, or runtime orchestration. Those mechanisms belong
to the crates that consume these contracts.

The rest of this file is the constitution for that split. It is normative for
this repository, and it is written to be read by whoever is about to add code
anywhere in the workspace, human or agent, before they write a function that
answers a question some other crate also answers.

## The design stance

Cook is built functional-core / imperative-shell, with this crate as the
mandatory shared kernel. A domain crate owns **state and effects**: it parses,
schedules, spawns, caches, renders progress. A **decision** — a pure function
from plain data to plain data whose answer more than one place must agree on —
is not the domain's to own. It is law, it is written once, and it lives in the
lowest crate its dependencies allow.

The enforceable rule is not "no logic in domains." It is:

> **No decision is implemented twice.**

Two implementations of one decision are two chances for the build to stop
meaning what the author wrote, and every one of them was once labelled a
deliberate copy by someone with a good reason that later expired.

## The admission bar

A thing belongs in `cook-contracts` when BOTH hold:

1. **Pure.** A function of its arguments, or plain data. No filesystem, no
   environment, no process, no clock, no mlua. This crate's dependency budget
   is serde/serde_json and stays that way.
2. **Shared law.** More than one crate — or more than one *phase* — must agree
   on the answer, and disagreement is a bug rather than a preference. An
   emitter and a consumer of the same literal. A composer and its inverse. A
   renderer and the hasher of the same bytes.

Be generous on scope: wire formats, magic-name constants, canonical
encodings, grammar, rendering rules, and classification enums all qualify.
Be strict on purity: the moment a candidate needs mlua or IO, it does not come
here, no matter how law-like it is.

Worked examples from this repo's history:

- **Moved here:** the sigil grammar and scanner (both phases must parse a
  placeholder identically); the substitution rendering (`sigil::subst` — the
  bytes in your command are the bytes in your hash); `shell_block::compose`
  (one rule for what a `{ … }` body means); `COOK_CMD_FAILED` (a wire format
  with two ends); `REGISTER_SURFACE_NAME` (an emitter/consumer literal);
  `Observation`/`OutputLog` and their canonical encoding; the registration
  summary (`registration::{RegisteredWorkspace, RegisteredRecipePub, …}`,
  COOK-428 — what registration declared, as data; producer, aggregator, and
  consumers all import the one definition, and the engine consumes it without
  a dependency on the crate that runs registration); the unit-level wiring
  plan (`unit_graph::plan`, COOK-402 / CS-0202 — which units become DAG nodes
  and which dependencies each has, with per-edge provenance; the engine lowers
  it into the DAG it executes and `cook why` renders it, replacing a
  hand-mirrored copy in the renderer that had drifted into a spec rule the
  Standard withdrew); `cache::recipe_cache_index_name` (the executor writing
  the index, `cook cache verify` reading it, and the graph's staleness check
  must resolve the same on-disk name).
- **Held out, correctly:** the Lua↔JSON value walkers (law, but mlua-bearing —
  their home is `cook-lua-stdlib`); executor scheduling, worker VM policy, the
  `CacheBackend` trait and its implementations (mechanism, not law).
- **Held out, WRONGLY, for a year:** `hash_str`. This list used to carry it as
  "law, but xxhash-bearing — its home is `cook-fingerprint`", which is the
  clearest statement of the mistake COOK-418 undid. xxhash is a dependency, not
  an effect; the bar below is about effects; and the crate invented to hold it
  went on to absorb 2,100 lines of cache IO. It lives in `hash` now. A worked
  example is only as good as the rule it illustrates, so when a rule changes,
  re-read the examples: this one outlived its own justification in the same
  file that states the justification.

## The stratum rule

"One source of truth" does not mean "one crate." It means **one home per
dependency stratum**, and a law lives as low as its dependencies allow:

| Stratum | Home | Budget |
|---|---|---|
| Pure law | `cook-contracts` | anything effect-free |
| Lua-touching law | `cook-lua-stdlib` | + mlua |

If a law's two consumers live in crates that both already depend on one of
these homes, the law goes there — full stop. Adding a small dependency edge to
reach the right home is almost always cheaper than the copy; the copy's cost
is deferred and silent, the edge's cost is visible in `Cargo.toml` once.

**The budget is about effects, not dependencies.** `layout.rs` fails the build
on `std::fs`, `std::env` and `std::process`; it says nothing about the
dependency list, and that is deliberate. A pure computation is admissible here
however it is spelled: `xxhash`, `sha2` and `globset` compute, they do not act.
What is inadmissible is reaching the world, and mutable process-global state
counts as reaching the world even when the grep comes back clean.

There used to be a third row, "Hashing/fingerprint law, `cook-fingerprint`,
+ xxhash". It was a mistake, and its failure is the best argument this document
has for its own rules. The row conflated a *dependency* with an *effect*: it
held `hash_str` out of this crate for needing xxhash, when the bar here has
never been about what a law imports. The stratum it named did not exist, so the
crate created to occupy it filled with the only thing adjacent: 2,100 lines of
cache IO, seven dependencies, `remove_file`, `remove_dir`, and a process-global
memo whose invariant is still owned by eight call sites in four crates. Nothing
was neglected. The boundary was fictional from the day it was drawn, and it
collected whatever had nowhere better to go.

`cook-fingerprint` no longer exists (COOK-418). Its effect-free half is here:
`consumes`, `context`, `envkey`, `evict`, `hash`, `pathlaw`, `cache::cas` and
`cache::step`. Its IO half is `cook-cache`, next to the backends and stores it
was always serving. The `CacheBackend` trait went with the IO rather than the
law: a trait definition would pass `layout.rs`, but it is the port to the
outside world, and a port belongs with its implementations.

One thing the dissolution did NOT fix, recorded so nobody assumes it did. The
stat memo (`cook_cache::statmemo`) is still armed and disarmed by convention
across eight call sites in four crates, because `cook-shell`, the crate that
spawns the commands that write the files, depends on `cook-contracts` alone and
refuses the edge. Moving the memo relocated the hazard correctly; making it
structural is COOK-400's problem shape and still open.

The lesson generalises, and it is the counterweight to everything else in this
document: **name boundaries, not computations.** A boundary you can observe
(disjoint dependency sets, a value handed across a phase line) enforces itself
once drawn. A boundary you reason your way to, ahead of the evidence, becomes an
attractor for the code that fits nowhere. Split where the seam already is.

## The deliberate-copy protocol

Sometimes an edge is genuinely refused (a crate that must stay dependency-free,
a context that may not touch the filesystem). A deliberate copy is then
permitted under exactly these conditions, all three:

1. **The comment names every rejected home**, not just one. "Cannot live in
   cook-contracts (mlua)" is insufficient if `cook-lua-stdlib` exists — a
   copy justified against an incomplete list of alternatives is a copy whose
   justification silently expires when a new home becomes eligible. This is
   not hypothetical: the Lua↔JSON twins were justified against contracts only,
   and the moment CS-0123 made `cook-lua-stdlib` a shared dependency of both
   sides, the comments were wrong and nobody could tell.
2. **An agreement test replaces the missing edge.** The two copies are run
   over the same inputs and asserted identical, in a test that lives where
   both are visible. A copy without an agreement test is drift with a delay.
3. **The copy is findable.** It names its twin by crate and path, so the
   next audit's grep lands on it.

When any listed rejected home stops being valid, the copy is dead on arrival:
consolidate before building on it.

## The standing agreement-test rule (COOK-361)

An agreement test that is trivially true is a unified path — that is the goal
state, and such tests are kept as regression guards against re-forking.
Whenever a second implementation of anything is *deliberately* introduced, a
new agreement test lands beside it in the same commit. Corollary: a parse-only
conformance fixture MUST NOT be cited as evidence that a surface works; pin
the fact, not the syntax.

## Enforcement

Structure does not hold itself; the hooks do. Cook's source of truth is a
trinity, and the tooling keeps its three parts in agreement:

1. **The Standard** (`standard/`) states the law in prose, spec-first: a
   commit touching language-surface crates pairs with its spec change in the
   same commit, enforced pre-commit.
2. **This crate** states the law as code, once.
3. **The conformance corpus** (`cli/e2e-fixtures/` and the law-level unit
   tests here) pins the two to each other — and every new fixture is verified
   to bite by mutation before it is trusted.

When you find yourself about to write a function this file's rules cover:
grep first. The law you need probably has a name already, and if it does not,
it wants to be born here rather than where you are standing.

### Does it come with a check?

Ask this of every structural decision before making it, and prefer the option
that answers yes.

The two purity rows above were written with equal authority in this same file.
One held for the life of the crate; the other drifted to seven dependencies and
two `remove_*` calls without anyone noticing. The difference was not intent,
seniority, or how carefully the rule was argued. It was `tests/layout.rs`: forty
lines that fail the build. Prose asks; a test refuses.

So, in descending order of what actually holds:

1. **A type that makes the wrong thing uncompilable.** `CachedTestResults` and
   `BlockedTestResults` are newtypes for exactly this reason (COOK-395): the two
   accumulators used to be one type, so transposing them anywhere compiled clean
   and silently filed cache hits as blocked.
2. **A crate boundary.** You cannot call across it without a `Cargo.toml` edge
   that shows up in review. This is why a crate split is a real investment and
   not tidying, and why one drawn on a fictional seam is expensive to undo.
3. **A test that fails the build**, like `layout.rs` or an agreement test.
4. **A comment.** Load-bearing only until the code beside it changes. This
   audit found four comments that contradicted the function directly beneath
   them, two of which had hidden a live bug from review for months.

A structural change that ships with nothing below rank 3 is documentation with
extra steps. That is not an argument against writing it down; it is an argument
for not believing it will hold on its own.

This matters more than it used to. Code arrives faster than review can read it,
and the failure mode is no longer a bad function: it is a second correct one.
Half the findings of the 2026-08-01 crate-charter audit were one decision with
two homes. None of them was a mistake at the moment it was written.

## Lineage

For the curious: this stance is functional-core/imperative-shell (Bernhardt)
with the domains pushed toward sans-IO state machines, plus a DDD shared
kernel made mandatory for law instead of discouraged for coupling. The
departure from textbook DDD is deliberate: bounded contexts that own private
models and translate at their edges assume a reviewer culture that carries
the translation in its head. Agentic development has no tribal memory — so
agreement must be structural, greppable, and machine-checked.
