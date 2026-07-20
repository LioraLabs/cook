# Sharing a cache across a team

cook's shared store is a content-addressed directory. If your team already has a
filesystem everyone can reach — an NFS mount, an SMB share, a colo volume, a
CI-mounted disk — you have everything you need. There is no server to run.

This guide covers the shared-directory setup end to end: the config, the one
correctness step that matters, and the operational edges worth knowing before you
point ten engineers at the same path.

The model is the same one described in [Caching and cache trust](../document.md):
every cacheable unit has one content-addressed key, and the local cache and the
shared store are addressed by that *same* key. Sharing a cache is therefore not a
mode — it's the ordinary cache with its directory pointed somewhere everyone can
see.

## The setup

Create `.cook/cloud.toml` next to your Cookfile:

```toml
[cache]
cache_dir = "/mnt/team/cook-store"
```

That's the whole thing. Absent this file, cook uses a per-user store under
`~/.cache/cook/cloud`; with it, cook uses the path you name. The store is
self-organizing — content-addressed entries sharded two levels deep — so you
never create or manage anything inside it. Point a second checkout at the same
path and it will fetch what the first one published.

You do **not** need a `[cloud]` section. That section selects the HTTP backend and
is a different deployment (see [What's next](#whats-next)); for a shared
directory, `[cache] cache_dir` alone is the entire surface.

Commit the file if everyone mounts the store at the same absolute path. See
[Operational notes](#operational-notes) if they don't.

## Seal your toolchain first

This is the one step that is not optional, and it is a correctness issue rather
than a tuning issue.

cook folds **only what you declared** into a key. It infers no machine identity,
toolchain, or locale of its own. That is what makes artifacts portable by
default — and it means that on a single machine, where the toolchain is a
constant, an under-declared key is invisible. Point that same key at a shared
store and the constant stops being constant: two machines with different
compilers compute the *same* key for *different* artifacts, and one fetches the
other's object files.

So before you share, name your real determinants:

```
probe compiler
    tools { cc }

recipe app
    ingredients "src/*.c"
    seal compiler
    cook "build/$<in.stem>.o" { cc -c $<in> -o $<out> }
```

A `tools { cc }` probe resolves `cc` on `PATH` and records a SHA-256 of the
executable's **contents** — not its path, not a `--version` string. `seal compiler`
folds that identity into each unit's key, so a teammate on a different compiler
gets a clean miss instead of your bytes.

Two consequences worth internalizing:

- **Content hashing is more forgiving than version strings.** A toolchain that
  moves on disk, or a package bump that leaves the binary byte-identical, still
  hits.
- **It hashes the binary you named, not its closure.** On a distro where
  `/usr/bin/gcc` is a thin driver, `cc1plus` can change underneath a
  byte-identical driver, and `tools { cc }` will not notice. On Nix-style
  toolchains, where the thing on `PATH` is a wrapper with the store path of the
  real compiler baked into it, hashing those bytes captures the closure
  transitively — the probe is at its most trustworthy exactly there.
- **Sharing is exactly byte-equality.** Two machines share sealed artifacts
  precisely when their toolchain bytes match — containers, one distro rolled
  out to a fleet, Nix pins. The same nominal version packaged by two distros
  hashes differently and does not share, and that is the correct outcome: its
  codegen can differ. Location never matters — homebrew, `/usr/bin`, and a
  home-dir toolchain with identical bytes are one identity.

If you're on `cook_cc`, it seals its own toolchain probe under the module
contract, and you get this without writing the probe yourself.

To check your work, run `cook cache verify` in CI on a *different host* than the
one that populated the store. It re-runs cached steps and reports byte divergence
under a matching key, which is how an undeclared determinant surfaces. It is a
diagnostic, not a gate.

## Who publishes

By default every client both fetches and publishes. For a team on a trusted
mount that is usually what you want: the first person to build something pays for
it once.

To make a client read-only, pass `--no-publish` or set `COOK_NO_PUBLISH=1`. It
still fetches by key; it just never uploads.

The `[cloud] publish = false` config key does the same thing persistently, but
note the direction it composes: **the flag can only turn publishing off, never
back on.** A committed `publish = false` cannot be overridden by CI. So if you
want the common "CI writes, laptops only read" posture, leave `publish` out of the
committed config entirely and have developers set `COOK_NO_PUBLISH=1` in their
shell or direnv. CI then publishes by simply not setting it.

## What never reaches the store

Three per-step dispositions opt out of sharing, and they mean different things:

- `local` — cached on this machine, never published. For genuinely
  machine-specific work.
- `pinned` — fetch-only. A cold miss is a hard error rather than a rebuild, which
  is how you assert "this must come from the store."
- `nondet` — the work is intrinsically non-reproducible (an LLM call, a
  timestamp), so cook records the output once and reuses the recording rather
  than pretending the bytes are deterministic. Recordings *are* shared.

Everything unannotated publishes after a run and fetches by key before running.

## Operational notes

**Every machine needs the same absolute path.** `cache_dir` is a literal path —
no `~` expansion, no environment-variable interpolation. A mixed-OS team where
Linux mounts `/mnt/team/cook-store` and macOS mounts `/Volumes/cook-store` cannot
share one committed config. Options: standardize the mountpoint (a symlink is
enough), or gitignore `.cook/cloud.toml` and let each machine set its own.

**An unreachable store is a warning, not a failure.** cook health-checks the
backend at startup and, if it's down, logs `cache backend unavailable` and
continues with the backend disabled. A flaky mount degrades you to local builds
rather than breaking the build.

**A hostile store cannot corrupt you.** Restored bytes are re-fingerprinted
against the key as they stream in; a corrupt or tampered entry is treated as a
miss and rebuilt. You do not have to trust the filesystem's integrity for the
build to be correct.

**Cap the artifacts you don't want to ship.** `[cloud] max_artifact_mib` bounds
what gets stored, and is honored by the directory backend.

**Nothing garbage-collects the store.** There is no retention policy, no eviction,
and no `cook cache prune`. The directory grows forever. It's ordinary files, so a
`find -atime` cron job is a perfectly reasonable stopgap — but budget the disk,
and don't be surprised by it.

**Permissions are the mount's problem.** Entries are ordinary files created by
whoever ran the build; if your team needs group-writability, set that up on the
mount (setgid, umask) the way you would for any shared directory.

## Proving it works

`cook why <recipe>` is read-only and classifies each unit against the store, so
it's the fastest way to confirm a store is live:

```console
$ cook why app
... [HIT (shared)]
```

`[HIT (shared)]` means the artifact came from the store by key.
`[MISS (local-only)]` means the step is `local` and never looks. On a miss, `cook
why` diffs your key against what the cached artifact was actually built from,
which is usually enough to spot the determinant you forgot to seal.

Two runnable end-to-end demos live in
[`examples/10-cache-trust/`](../examples/10-cache-trust/):

- `share-local.sh` — two checkouts on one host against one shared store, asserting
  each disposition's behavior across a simulated host change.
- `share-docker.sh` — the same, across two containers.

Both are the setup in this guide, scripted.

## What's next

A shared directory covers teams with shared infrastructure. It does not cover a
distributed team with no common filesystem, and it has no answer for retention,
access control, or cross-team hit analytics — the things a directory
fundamentally can't do.

A hosted store is the natural next step, and the client is already wired for it:
`[cloud] enabled`, `endpoint`, and `project`, plus a `COOK_CLOUD_API_KEY`, select
an HTTP backend against the same keys and the same dispositions. What's missing is
the service on the other end, which is a question of whether enough people are
sharing caches to justify operating one. If that's you, say so — demand is what
decides the timing.

Until then, the shared directory is the supported path, and it is the same cache.
