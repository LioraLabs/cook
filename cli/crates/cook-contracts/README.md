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
  `Observation`/`OutputLog` and their canonical encoding.
- **Held out, correctly:** the Lua↔JSON value walkers (law, but mlua-bearing —
  their home is `cook-lua-stdlib`); `hash_str` (law, but xxhash-bearing — its
  home is `cook-fingerprint`); executor scheduling, worker VM policy, cache
  backends (mechanism, not law).

## The stratum rule

"One source of truth" does not mean "one crate." It means **one home per
dependency stratum**, and a law lives as low as its dependencies allow:

| Stratum | Home | Budget |
|---|---|---|
| Plain data + pure rules | `cook-contracts` | serde only |
| Lua-touching law | `cook-lua-stdlib` | + mlua |
| Hashing/fingerprint law | `cook-fingerprint` | + xxhash |

If a law's two consumers live in crates that both already depend on one of
these homes, the law goes there — full stop. Adding a small dependency edge to
reach the right home is almost always cheaper than the copy; the copy's cost
is deferred and silent, the edge's cost is visible in `Cargo.toml` once.

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

## Lineage

For the curious: this stance is functional-core/imperative-shell (Bernhardt)
with the domains pushed toward sans-IO state machines, plus a DDD shared
kernel made mandatory for law instead of discouraged for coupling. The
departure from textbook DDD is deliberate: bounded contexts that own private
models and translate at their edges assume a reviewer culture that carries
the translation in its head. Agentic development has no tribal memory — so
agreement must be structural, greppable, and machine-checked.
