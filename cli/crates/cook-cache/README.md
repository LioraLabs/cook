# cook-cache

`cook-cache` stores a build's cache state and hands it back byte-identical.

Two stores, one job: the per-recipe step index (`.cook/cache/<recipe>.idx`,
what ran and against which recorded inputs and outputs) and the
content-addressed artifact store (the bytes those steps produced, local or
remote). It writes both, reads both, and decides nothing about whether what it
returns is still valid.

## How it does that well

- **Two tiers, one trait, one verifier.** `LocalBackend` (filesystem CAS) and
  `CloudBackend` (sync HTTP over the v1 wire protocol) both implement
  `CacheBackend`, which is defined upstream in `cook-fingerprint` so neither
  implementation can bend it toward itself. `VerifyingReader` is shared, not
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
  `cook-fingerprint::evict`, shared with the eventual cloud-side sweep.
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

It does not decide whether an entry is still valid. `needs_rebuild_cook`, key
composition (`cloud_key` / `artifact_key`), the env denylist, probe
fingerprints, and the restore-into-workspace step all live in
`cook-fingerprint`; this crate stores and serves what those decisions address.
It does not own eviction *policy*, only candidate enumeration and plan
application. It does not own the meaning of what it stores: `Observation`,
`CacheMeta`, and the index-basename encoding are `cook-contracts`. It does not
schedule, print, or emit progress; a lookup returns a value and the caller
decides what to say about it.

## Where the boundary is soft

Named rather than hidden, because the seam moves and a stale claim is worse
than none:

- `lib.rs` re-exports a dozen `cook-fingerprint` items (`check`, `envkey`,
  `context`, `needs_rebuild_cook`, `hash_file`, `RestoreCtx`, ...) for
  back-compat with call sites that predate the split. New code should import
  from `cook_fingerprint` directly. The re-exports make the boundary read as
  softer than it is, and five of the integration tests under `tests/` exercise
  `needs_rebuild_cook` through them.
- `depfile.rs` parses Make-format `.d` files. It is the one module here that
  neither writes nor reads cache state; it lives here because its output feeds
  the records that do.
- `parse_size` and `SIZE_LITERAL_HELP` are pure and shared with `cook-cli`'s
  `cache gc --max-size`, which by the `cook-contracts` admission bar puts their
  home upstream, not here.
