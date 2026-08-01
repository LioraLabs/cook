# cook-fingerprint

`cook-fingerprint` computes a unit's cache identity: the content hashes and
keys that say what its last run depended on, and whether the world still
matches them.

## How it does that well

- **The judgment is not implemented here.** Comparing recorded determinants
  against current ones is a pure rule over three `u64`s, so it lives in
  `cook_contracts::cache::record::determinant_drift` and this crate only maps
  the answer to a `RebuildReason`. `needs_rebuild_cook` was never a check: it
  compares, then stats and hashes, then downloads and writes. COOK-360 named
  those three phases (judge / observe / repair), pushed the one pure phase
  upstream, and deleted the test store's hand-rolled copy of it.
- **One function judges every unit kind.** `needs_rebuild_plate` was
  `needs_rebuild_cook` with the output half removed: same predicates, same
  order, same `check_inputs` call, and no production caller. CS-0186 gave
  observing units one record under one key, so the general function judges them
  with an empty output slice. The copy was deleted rather than rewired;
  `check.rs:899` records why, so nobody reintroduces it.
- **`hash_file` and `hash_reader` are deliberately co-located.** `cook why`
  predicts a not-yet-restored output's hash by draining the shared store's
  stream, and that prediction is sound only while the path version and the
  reader version agree byte for byte. A change to one that missed the other
  would compile cleanly and silently mispredict every downstream key (CS-0173).
- **Nothing re-reads a string to decide what it is.** `resolve_declared_inputs`
  expands only entries the declaration already marked as patterns. Sniffing for
  `*`, `?` and `[` here is what §17.1.1.2 forbids: those are ordinary filename
  characters, so a real `pages/[id].tsx` would expand, match nothing, and vanish
  from the key. It would vanish on the recording side too, so the two sides
  agree and the unit hits over a file that changed (CS-0186).
- **Order is itself a determinant.** The resolved input list keeps declaration
  order and drops duplicates rather than sorting, because `check_inputs`
  compares recorded against current element-wise. Sorting here would read as an
  input-set change on every unit in the project.
- **Narrowing fails toward over-keying.** A `consumes` allowlist that matches
  nothing returns the unnarrowed candidate set, never the empty one
  (`consumes.rs:84`). A filter that empties the fold replays a stale pass;
  over-invalidating only costs a rebuild (CS-0175).
- **The stat memo has one blunt invariant instead of an invalidation scheme:**
  it serves only values read before cook wrote anything, and the first write of
  any kind disarms it permanently. DuckDB's index holds 648,153 input records
  naming 8,350 distinct paths, so validating a settled build issued 648k stats
  where 8.4k would do (0.88s versus 0.01s). `statmemo.rs` also names the three
  spawn sites that deliberately have no disarm hook, and why adding one would be
  dead code bought at the price of a spec-pairing requirement (COOK-306).
- **The check does not re-read depfiles.** A discovered-inputs unit records the
  fat input set at execution time, so the stored entry already *is* the
  discovered set; re-parsing the `.d` recovers information already in hand, and
  both a changed and a deleted header are still caught by the recorded-input
  walk. On DuckDB that removed 1,705 files and 17.15 MB of text yielding 330,415
  tokens from every run, including runs that executed nothing (COOK-313,
  `check.rs:271`).
- **`recipe_namespace` is composed in exactly one function.** `cloud_key`,
  `ArtifactMeta` and `DeterminantManifest` all carry that string, and any two of
  them drifting is a silent cache split rather than an error. `cloud_key` also
  writes a `0x00` delimiter between the namespace and the hash bytes, so no
  namespace can be crafted to collide with a hash prefix.
- **A candidate key is validated against the store, never assumed.** For depfile
  units `fetch_by_key` re-hashes each recorded discovered-path set at its
  *current* content and probes the resulting full key; a set whose files moved
  composes a different key and naturally misses. That is what makes a warm
  revert restorable instead of a permanent refetch loop (COOK-278).
- **Restored bytes are pinned to the local record before they touch the
  workspace.** The sidecar's `VerifyingReader` cannot catch a backend that
  rewrote both the bytes and the sidecar; on the warm path there is a
  locally-trusted `StepEntry` hash to pin against, so `restore_one` verifies
  first and only then writes tmp-and-rename (CS-0054 §2, spec §8.6).
- **The probe fingerprint's version marker is inside the hash.** CS-0102
  re-encoded probe values; bumping `COOK_PROBE_FP_V1` to `V2` made every
  artifact written before it an unreachable key rather than a wrong hit.
- **A tool's identity is its content hash, not where it resolved.** The path is
  location metadata, excluded from the fingerprint and deliberately not
  memoised so a reader always sees where the tool is *now*. The binary hash is
  memoised, because five recipes sealing one `web:tools` probe otherwise re-read
  a 60 MB `node` five times (CS-0157, CS-0158).

## What it does not do

It does not store anything. `cook-cache` owns `LocalBackend`, the binary index
encoding, the store layout, and the CAS on disk; this crate owns only the
`CacheBackend` trait those implement and the contract text that says what a
conforming `get`/`put` must guarantee.

It does not decide what a unit's inputs *are*. That is settled at declaration
time by the phase that knew where each entry came from; this crate expands the
patterns that were already marked as patterns and hashes the result.

It does not schedule, print, emit progress, or classify a failure. It returns a
`RebuildResult` with a reason, and the caller decides what to say about it.

## Relationship to `cook-contracts`

`cook-contracts` owns what the recorded values MEAN: `Determinants`,
`UnitRecord`, `RECORD_SCHEMA_VERSION`, `DeclaredInput`, `Observation`, and the
one lowercase-hex encoder (COOK-392). Its own layout test forbids it stateful
standard-library access, so it can state the rebuild rule but can never observe
a file to apply it. This crate observes, and then repairs. The dividing question
is whether answering requires touching disk, PATH, or a backend.

## Where this crate exceeds its stratum

The constitution names this crate a stratum home: *hashing/fingerprint law,
budget `+ xxhash`*. It does not currently hold that role cleanly, and the gap is
recorded here rather than left for the next audit to rediscover.

- The real dependency budget is xxhash **plus** sha2, `which`, `glob`,
  `globset`, `serde_json` and `tracing`.
- It writes and deletes. `restore_one` (`check.rs:539`) materialises artifacts
  into the workspace; `reconcile_dir_output` (`lib.rs:284`) removes stray files
  and prunes directories. A law crate that sweeps a build tree is a mechanism
  crate wearing the wrong name.
- `BackendConfig` (`backend.rs:26`) carries network timeouts, retry counts,
  backoff bounds and a quota hint. Nothing in this crate reads any of it; only
  `cook-cache` constructs one.
- `evict.rs` is pure store-GC policy, written explicitly to be reusable
  server-side. By the admission bar it belongs in `cook-contracts`; the only
  thing holding it here is that `EvictCandidate` and `CloudKey` live in
  `backend.rs`.
- Two private lexical path normalisers with identical semantics and different
  names: `lexically_normalize` (`lib.rs:247`) and `normalize_lexical`
  (`check.rs:434`). One decision, two implementations, inside one crate, so the
  deliberate-copy protocol does not even apply.
- Two functions named `hash_file` with different algorithms and different return
  types: xxh3 to `u64` (`check.rs:44`, public) and SHA-256 to `[u8; 32]`
  (`probe.rs:118`, private).
- Two per-run memos with two different safety disciplines. `statmemo` disarms on
  any cook write; `memoized_hash` (`probe.rs:105`) never invalidates, on the
  argument that one run is one process. The asymmetry is real and undocumented
  at the second site.
