# cook-cache

`cook-cache` decides whether a unit's answer is already known, and if it is,
puts that answer back on disk byte for byte.

Two stores under that: the per-recipe step index (`.cook/cache/<recipe>.idx`,
what ran and against which recorded inputs and outputs) and the
content-addressed artifact store (the bytes those steps produced, local or
remote).

COOK-418 widened the sentence. This crate used to store and hand back, and
judged nothing; it now also owns the judging, because roughly 1,900 lines
arrived from `cook-fingerprint` when that crate was dissolved. What did NOT
arrive is the part that needs no world: key composition, determinant drift,
eviction policy, and what a declared path IS are pure rules in
`cook-contracts`, and this crate calls them with real inputs.

## How it does that well

- **Two tiers, one trait, one verifier.** `LocalBackend` (filesystem CAS) and
  `CloudBackend` (sync HTTP over the v1 wire protocol) both implement
  `CacheBackend`, defined once in `cas_backend` so neither implementation can
  bend it toward itself. `VerifyingReader` is shared, not
  copied: the same SHA-256 tee guards a `File` and an HTTP body.
- **Verification streams; nothing is buffered to prove it.**
  `VerifyingReader` tees bytes through the hasher and raises `InvalidData` at
  EOF. The in-memory alternative would materialize a multi-GB artifact into a
  `Vec<u8>` before it could be trusted, which is the OOM path CS-0056 exists to
  close.
- **Every read fails closed, and a failure is a miss.** Missing sidecar,
  malformed sidecar, sidecar without bytes, a pre-CS-0054 zero-sentinel
  `content_hash`, a streaming hash mismatch: all surface as `Ok(None)` and
  force a rebuild. Untrusted bytes are never installed, and a regeneratable
  entry never fails a build.
- **`put` is idempotent and refuses conflicts rather than overwriting**
  (CS-0055). Re-putting identical bytes under a key is a no-op that still
  stamps the canonical hash; putting *different* bytes under an existing key is
  an error. A caller-claimed `content_hash` that disagrees with the streamed
  bytes is also refused, so a sidecar inconsistent with its blob cannot be
  written.
- **Nothing partial is ever visible.** Blob, `meta.json` sidecar,
  `provenance.json` manifest, and the recipe index all commit by write-temp
  then rename. `put_manifest` builds its temp path with `with_file_name`
  instead of `with_extension`, because the manifest path already ends in
  `.json` and `with_extension` would have replaced that segment and orphaned
  the temp file under a mangled name.
- **The index format was chosen by measurement, not taste** (CS-0166,
  COOK-313). The v6 TOML index on a 1,711-node DuckDB build reached 69 MB, of
  which 48.8% was `toml`'s array-of-tables restating each step key once per
  input record, and whose 328k input records resolved to 6,730 distinct paths.
  Parsing it cost 0.75s on a run that executed nothing. `index_bin` interns
  every path once per index and stores records in one flat pool that steps
  slice into.
- **`index_bin::decode` is total.** Every length and offset read out of the
  payload is bounds-checked before use, so a truncated, resealed, or hostile
  `.idx` yields `Err(DecodeError)` rather than a panic or an out-of-bounds
  read. The xxh3 payload checksum catches accident; the bounds checks exist for
  the cases it does not.
- **No migration, ever.** The index is regeneratable by definition, so a
  format change deletes rather than converts: `sweep_superseded_indexes` drops
  pre-v4 `.bin`, v4..v6 `.toml`, and torn temps on first touch of a cache dir.
  It is a denylist of known-superseded extensions rather than an allowlist of
  `.idx` because the cache dir is not exclusively Cook's; `cook_cc.json` sits
  in every cc-built project's `.cook/cache/`, and an allowlist sweep would have
  deleted it.
- **The hot path hands out one entry, not the index** (COOK-306). DuckDB's
  128 MB / 648k-record index, copied once per work node, was 95% of all
  allocation traffic and ~102s of a 107s settled no-op. `lookup_step` copies a
  single `StepEntry`; `get_or_load` returns an `Arc`; `update_step`
  short-circuits when the entry being written equals the stored one, because a
  settled run rewrites what it just read and marking the recipe dirty costs a
  full re-serialization of the whole index.
- **Encoding is deterministic, and two optimizations depend on it.** Path ids
  are assigned in sorted order rather than first-encounter order, so insertion
  history cannot leak into the file. The dirty-set flush and the
  compare-before-store short-circuit above are only sound because an unchanged
  index produces unchanged bytes.
- **The dangerous half of eviction is off the trait** (milestone D2).
  `enumerate` and `apply_eviction` are inherent methods on `LocalBackend`, not
  `CacheBackend` methods, so a `Box<dyn CacheBackend>` pointed at a shared
  multi-tenant store can never acquire "list every object" or "delete these".
  The policy that picks victims (`plan_eviction`) is pure and lives upstream in
  `cook_contracts::evict`, shared with the eventual cloud-side sweep.
- **Freed bytes are counted from the delete's own result, not from a
  preceding stat.** Stat-then-delete opens a window where a concurrent sweep
  removes the blob in between and both sweeps report the same bytes freed;
  deriving "removed" from `remove_file`'s return closes it, and also stops
  counting a blob whose removal failed for a permission error and is still on
  disk.
- **LRU's last-access signal is the blob's own mtime**, restamped with one
  `utimensat` per hit (COOK-233). The rejected alternative, a `last_access`
  field inside the sidecar, puts write amplification on the hot read path. The
  touch is inert only because restore is a byte copy rather than a hardlink, so
  the comment states that argument and forbids the change that would silently
  break input-freshness detection.
- **Index filenames go through the encoder that sits beside its inverse.**
  Recipe names may contain `/`: a module-minted recipe like `@cap/env:build`
  used to write into a directory that never existed, and the ENOENT was
  swallowed, so the recipe simply never cached (COOK-273). The percent-encoder
  lives in `cook_contracts::layout` next to the decoder that `cook-engine` uses
  to read the same names back (COOK-393).

## What it does not do

It does not compose a cache key or state a rule that needs no world. Key
composition (`cloud_key` / `artifact_key`), the env denylist, the probe
fingerprint fold, determinant drift, and what a declared path IS are
`cook-contracts`; this crate calls them with what the filesystem says. What it
DOES own, since COOK-418, is asking: `needs_rebuild_cook`, the restore step, and
the probe input resolution that reads env, PATH and files. It does not own
eviction *policy*, only candidate enumeration and plan application. It does not
own the meaning of what it stores: `Observation`,
`CacheMeta`, and the index-basename encoding are `cook-contracts`. It does not
schedule, print, or emit progress; a lookup returns a value and the caller
decides what to say about it.

## Where the boundary is soft

Named rather than hidden, because the seam moves and a stale claim is worse
than none:

- `lib.rs` re-exports a dozen `cook-contracts` items (`consumes`, `context`,
  `envkey`, `evict`, `hash_str`, `cache::cas`, ...) alongside its own, for
  back-compat with call sites that predate COOK-418. New code should import law
  from `cook_contracts` directly. The re-exports make the boundary read as
  softer than it is, and five of the integration tests under `tests/` exercise
  `needs_rebuild_cook` through them.
- Two file hashes live here and they are not interchangeable (COOK-414):
  `check::hash_file` is xxh3 over a path and answers LOCAL content identity:
  what a `FileRecord` carries, what the local key folds. Meanwhile
  `probe::hash_file_sha256` is the SHA-256 identity that LEAVES the machine in a
  probe fingerprint or a cloud key. They were both spelled `hash_file` until
  COOK-414, one publicly and one privately in the module that shadowed it. Both
  are now pinned to golden vectors computed outside this codebase, because the
  suite's determinism tests pass under any hash function and changing what
  either computes orphans every cache in existence.
- `depfile.rs` parses Make-format `.d` files. It is the one module here that
  neither writes nor reads cache state; it lives here because its output feeds
  the records that do.
- `parse_size` and `SIZE_LITERAL_HELP` used to be listed here as pure and
  shared with `cook-cli`'s `cache gc --max-size`, which by the `cook-contracts`
  admission bar put their home upstream. COOK-421 moved them to
  `cook_contracts::size`; `cloud_config.rs` imports them like anyone else, and
  the re-export cook-cli used to tunnel through is gone.

## What lives here that the sentence above does not cover

Said plainly rather than stretched to fit, per the crate-charter convention.

`statmemo` holds the crate's two per-run memos. The first memoises input
`mtime` under an arm/disarm discipline (COOK-306: a large C++ graph resolved
648,153 input records to 8,350 distinct paths, so validating a settled build
cost 0.88s of `stat` where 0.01s would do). The second memoises a resolved tool
binary's SHA-256, and revalidates on every lookup against everything one
`metadata` call says about the inode a path names (COOK-414). They sit together
and each doc states the other's rule, because the two disciplines look arbitrary
apart and are forced
apart on inspection: **a stat memo cannot revalidate itself**, because the
`stat` IS the cheap check it exists to avoid, **and a hash memo can**, for one
`metadata` call against a 60 MB read. Arm/disarm on the hash memo would be
strictly worse, since `disarm` fires on the first executed command and the
register phase, where module code calls `cook.tools.id`, runs entirely
disarmed.

The hash memo's residual window is named rather than asserted away, because it
sits on a false-hit path: it is exactly as discriminating as `metadata` is. On
unix that means a rewrite would have to reproduce mtime, ctime, length, inode
and device, which cook cannot do to itself. Mtime and length alone would NOT
have been enough: coarse filesystem timestamp granularity plus a same-length
relink is a real pair, which is why `touch_forward` exists in this module's
tests. Off unix only those two fields are available, and the README says so
rather than letting the unix case stand for both.

Both are correctly located here rather than in `cook-contracts`, because global
mutable state is not law however effect-free the grep looks. The stat memo's
invariant is still owned by convention at eight call sites across four crates,
and it cannot be enforced at the write site: `cook-shell`, which spawns the
commands that write the files, depends on `cook-contracts` alone and refuses the
edge. A known hole, not a design, and the reason the hash memo was given a rule
that needs no call sites to keep it.

`depfile` parses Make `.d` files. It neither reads nor writes cache state and
its only consumer is `cook-engine` (COOK-425).

## Lineage

`cook-fingerprint` was created to be the home for "hashing law", and the
stratum it named did not exist: the bar `cook-contracts` enforces is about
effects, not dependencies, so nothing was ever keeping a hash out of it.
Having been made for a boundary that was not there, it filled with the only
thing adjacent to computing a fingerprint, which was this crate's IO. Its
effect-free half is now in `cook-contracts` (`consumes`, `context`, `envkey`,
`evict`, `hash`, `pathlaw`, `cache::cas`, `cache::step`) and its acting half
is here.

The `CacheBackend` trait came here rather than to contracts. A trait definition
would have passed the purity test; it is the port to the outside world, and a
port belongs with its implementations.
