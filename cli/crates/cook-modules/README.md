# cook-modules

`cook-modules` installs the Lua modules a project declares, and records exactly
what it installed.

## How it does that well

- **The ask and the record are two files and never merge.** `cook.toml`'s
  `[modules]` is what the author asked for; `cook.lock` is the closure that
  answer produced, direct and transitive alike, each entry carrying the
  SHA-256 of the source rock it came from. A bare `cook modules install`
  installs from the lock, so the reproducible path cannot quietly resolve
  something the author never saw. Every state-changing invocation rewrites the
  lock; nothing else does.
- **It invents no constraint grammar.** A version constraint is passed to
  luarocks verbatim, and rock names use luarocks's own character set. A build
  tool that re-implemented a package manager's resolution rules would own two
  answers to "which version satisfies this", and the second one would be
  wrong on the first release that stretched the syntax.
- **Every rock lands in the project's own build-output tree.** Every driver
  invocation passes `--tree <project>/.cook/modules`, resolved through
  `cook_contracts::layout::modules_dir` (§27.1.1 / CS-0207) rather than
  spelled here, so the directory this crate installs into is the directory the
  module loader searches. A user-global luarocks tree is never written and
  never read.
- **Index precedence is positional, and an override is one invocation wide.**
  `[registry].indexes` becomes repeated `--server <url>` flags in
  left-to-right order; `--registry` prepends for the current command only and
  is not written to the manifest. Precedence you can read off the argv is
  precedence you can reproduce.
- **A luarocks failure arrives whole.** The driver captures argv, stdout and
  stderr and puts all three in the error, each stream bounded by
  `cook_contracts::CapturedStream` at the same 64 KiB the rest of Cook
  truncates command output at (SHI-188). Nothing parses luarocks's output into
  a summary, because a summary is exactly where the one line that explained
  the failure goes missing.
- **Determinism is a type choice, not a discipline.** Every serialised
  collection is a `BTreeMap`/`BTreeSet`, so two machines that installed the
  same closure write the same `cook.lock` bytes and a diff means something.
- **A digest it does not have is a sentinel, not a zero.** A rock whose source
  archive was absent from the cache at introspection time is recorded as
  `sha256-unknown`, and `verify_integrity` refuses to be called on one rather
  than comparing against a placeholder. "Unknown" and "verified" are the two
  answers that must never be confused, so they are not the same shape.

## What it does not do

It knows nothing about recipes, work units, the DAG, the cache, cache keys, or
a Lua VM, and it never loads a module — it only puts one on disk where the
loader will find it. It renders no progress and owns no exit-code taxonomy
beyond its own.

Its entire reach into the workspace is `cook-contracts`, for two items:
`layout::modules_dir` and `CapturedStream`. That disjointness is the evidence
this boundary is real rather than argued for — it is what let 1,392 lines leave
`cook-cli` for one `Cargo.toml` edge (COOK-420). The day this crate needs
`cook-engine` is the day to re-open where the line was drawn.

## Two things kept on purpose

- **`clap` lives here.** `ModulesArgs` and `ModulesCmd` are this crate's, not
  `cook-cli`'s. Splitting the flags from what they mean would put one decision
  in two crates, which is the shape `../cook-contracts/README.md` refuses; the
  surface's whole involvement is one `Cmd::Modules` variant and one call.
- **`run` returns an `i32` and prints `cook modules: …`.** It does not route
  through `cook-cli`'s `CookError`. `CookError::Other` carries a flat `String`
  and prints under a bare `cook:` prefix, so adopting it would replace an
  anyhow chain — "install cook_smoke: luarocks exited 1: …" — with its last
  link, and change the prefix users grep for. Consistency that costs a
  diagnostic is not an improvement.

## Where it falls short of that

**Integrity is recorded and never checked.** `verify_integrity` has no
production caller: `install_locked_closure` reads the lockfile, validates it
against the manifest, and installs each entry without comparing the source
rock's SHA-256 to the digest the lock records. The function, its
`has_known_integrity` gate and its tests are all correct; nothing calls them.
The first sentence of this charter says "records exactly what it installed",
and that is true — what is not yet true is that it installs exactly what it
recorded.

**Two trust flags are parsed and discarded.** `--non-interactive` and
`--accept-trust` are declared on `ModulesArgs` and read nowhere, so the TOFU
consent they name does not exist in either direction: nothing prompts, and
nothing to accept. A flag that accepts a risk the code never takes is worse
than no flag, because a CI pipeline passing `--accept-trust` reads as having
made a decision.

Both are recorded here rather than fixed, because the fix is behaviour and
this crate arrived by a move that changed none (COOK-420).
