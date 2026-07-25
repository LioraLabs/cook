//! End-to-end tests for `cook cache gc`, driving the real `cook` binary via
//! `CARGO_BIN_EXE_cook` against a real, on-disk, hand-seeded CAS.
//!
//! ISOLATION IS NON-NEGOTIABLE — this file's subject deletes files. Every
//! fixture writes its own `.cook/cloud.toml` with `[cache] cache_dir`
//! pointing at an absolute path inside the fixture's own `TempDir` *before*
//! the first `cook` invocation (`CloudConfig::resolved_cache_dir` takes a
//! configured `cache_dir` verbatim — `PathBuf::from`, no join onto the
//! project root, no canonicalize — so a relative path would resolve against
//! whatever the engine's current directory happens to be, not the fixture).
//! As a second, independent belt, every `Command` this file spawns also sets
//! `XDG_CACHE_HOME` into that same `TempDir` (`Fixture::run` /
//! `Fixture::run_with_env` do this unconditionally — there is no code path
//! in this file that can construct a `Command` without it). No test here
//! may read or write the developer's real CAS at `~/.cache/cook/cloud`.
//! Every destructive run additionally asserts the `Store:` line of its
//! output against the fixture's own `cache_dir` — since that path is itself
//! inside the fixture's `TempDir`, the assertion is simultaneously "gc
//! operated on the store we configured" and "gc never touched anything
//! outside this sandbox".
//!
//! Seeding writes directly into the CAS layout `LocalBackend` and
//! `EvictCandidate` describe (`cli/crates/cook-cache/src/backend.rs`,
//! `cli/crates/cook-fingerprint/src/backend.rs`): a blob at
//! `{cache_dir}/{2 hex}/{62 hex}`, a `.meta.json` sidecar built by
//! constructing `cook_engine::cook_cache::backend::ArtifactMeta` and
//! serializing it with `serde_json` (never hand-written JSON — the struct
//! has nine fields with no serde default, and `enumerate` silently degrades
//! a malformed sidecar to `(kind: None, recipe_namespace: "")`, which would
//! quietly turn a seeded exempt-kind object into an evictable `file` and
//! make the exemption tests pass for the wrong reason), and a
//! `.provenance.json` sidecar. The blob's mtime is stamped with the stable
//! `std::fs::File::set_times` / `std::fs::FileTimes` API (no `filetime` dev
//! dependency), which works on a read-only `File::open` handle on this
//! toolchain.
//!
//! There is no literal `"file"` kind string anywhere in production code:
//! plain-file artifacts carry `kind: None`, which `cook cache du` folds into
//! its `"file"` display row. "Seed a `kind=file` blob" below always means
//! `ArtifactMeta.kind == None`.
//!
//! Output shape reminder (see `cache_gc.rs`'s renderers): a report with at
//! least one victim is THREE lines (`Store: …`, then `Would free: …` /
//! `Freed: …`, then a second, DIFFERENT `Store: … -> … (N objects -> M)`
//! transition line); a report with no victims is exactly TWO lines
//! (`Store: …` then `Nothing to evict.`). Assertions below index lines
//! positionally rather than grep for `^Store:`, since two different lines
//! both start with that word.
//!
//! Sizes are seeded as round decimal values (multiples of 1000 / 1_000_000)
//! so `human_size`'s one-decimal-place SI rendering is exact and
//! unambiguous — never an `X.X5` boundary a floating-point round could tip
//! either way.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

use cook_engine::cook_cache::backend::ArtifactMeta;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// Write `.cook/cloud.toml` with `[cache] cache_dir = <cache_dir>` (verbatim,
/// absolute). The isolation boundary every fixture in this file goes
/// through before the first `cook` invocation.
fn write_local_cloud_toml(project_dir: &Path, cache_dir: &Path) {
    fs::create_dir_all(project_dir.join(".cook")).unwrap();
    let toml = format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy());
    fs::write(project_dir.join(".cook/cloud.toml"), toml).unwrap();
}

/// One isolated fixture: its own scratch `TempDir` containing a project
/// directory, a (not-yet-created) CAS `cache_dir`, and an `xdg_dir` used
/// only as the `XDG_CACHE_HOME` belt-and-suspenders. `_scratch` is kept
/// alive for the fixture's lifetime purely so its `Drop` doesn't clean the
/// directory out from under a running test; nothing reads it directly.
struct Fixture {
    _scratch: TempDir,
    project_dir: PathBuf,
    cache_dir: PathBuf,
    xdg_dir: PathBuf,
}

impl Fixture {
    /// A bare project — `Cookfile` (so entry-point discovery succeeds; `gc`
    /// itself never registers the workspace, exactly like `du`) plus a
    /// local `.cook/cloud.toml` pointing `[cache] cache_dir` at this
    /// fixture's own, not-yet-existing `cache_dir`. Seed via `seed` before
    /// running `gc` in any test that expects a non-empty store.
    fn new() -> Self {
        let scratch = TempDir::new().unwrap();
        let project_dir = scratch.path().join("project");
        let cache_dir = scratch.path().join("cas");
        let xdg_dir = scratch.path().join("xdg");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("Cookfile"),
            "recipe build\n    test { true }\n",
        )
        .unwrap();
        write_local_cloud_toml(&project_dir, &cache_dir);
        Self {
            _scratch: scratch,
            project_dir,
            cache_dir,
            xdg_dir,
        }
    }

    /// Overwrite `.cook/cloud.toml` with `[cloud] enabled = true` (plus the
    /// endpoint/project `CloudConfig::load_or_default` requires once
    /// enabled) still naming this fixture's own `cache_dir` — isolation is
    /// non-negotiable even on a path gc's cloud-enabled branch is expected
    /// to never touch.
    fn enable_cloud(&self) {
        let toml = format!(
            "[cloud]\nenabled = true\nendpoint = \"https://example.invalid\"\nproject = \"myproj\"\n\n\
             [cache]\ncache_dir = {:?}\n",
            self.cache_dir.to_string_lossy()
        );
        fs::write(self.project_dir.join(".cook/cloud.toml"), toml).unwrap();
    }

    /// Seed one CAS object: blob + `.meta.json` (via `ArtifactMeta` +
    /// `serde_json`, never hand-written) + `.provenance.json`, then stamp
    /// the blob's mtime to `age_secs` in the past. `counter` must be
    /// distinct per object in a test; it's the sole input to the object's
    /// key.
    fn seed(
        &self,
        counter: u32,
        size: u64,
        age_secs: u64,
        kind: Option<&str>,
        recipe_namespace: &str,
    ) -> [u8; 32] {
        seed_object(&self.cache_dir, counter, size, age_secs, kind, recipe_namespace)
    }

    /// Every `Command` this file spawns goes through here (or
    /// `run_with_env`), both of which unconditionally set `XDG_CACHE_HOME`
    /// into this fixture's own `TempDir` — the second isolation belt beyond
    /// the configured `[cache] cache_dir`.
    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(cook_bin());
        cmd.args(args).current_dir(&self.project_dir);
        cmd.env("XDG_CACHE_HOME", &self.xdg_dir);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.output().expect("run cook")
    }

    /// Run `cook cache du` and return the set of `By kind:` labels it
    /// reports. Used as a seeding sanity check: if a sidecar was
    /// mis-constructed, `enumerate` silently degrades it to `kind: None`,
    /// and this would fail loudly (missing an expected label) rather than
    /// letting an exemption test pass for the wrong reason.
    fn kinds_reported(&self) -> std::collections::BTreeSet<String> {
        let du = self.run(&["cache", "du"]);
        assert!(du.status.success(), "cache du must succeed: {du:?}");
        let text = String::from_utf8(du.stdout).unwrap();
        parse_kind_labels(&text)
    }
}

fn parse_kind_labels(du_text: &str) -> std::collections::BTreeSet<String> {
    let mut set = std::collections::BTreeSet::new();
    let mut in_section = false;
    for line in du_text.lines() {
        if line == "By kind:" {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.is_empty() {
            break;
        }
        if let Some((label, _)) = line.trim_start().split_once(": ") {
            set.insert(label.to_string());
        }
    }
    set
}

/// Turn a small counter into a distinct, valid 32-byte key. Only the low
/// bytes vary, which is fine: distinctness (not distribution) is all any
/// test here needs.
fn key_from_counter(counter: u32) -> [u8; 32] {
    let mut key = [0u8; 32];
    key[28..].copy_from_slice(&counter.to_be_bytes());
    key
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The three on-disk paths for one CAS object: blob, `.meta.json`,
/// `.provenance.json`. Mirrors `LocalBackend::path_for` +
/// `with_extension("…")`, independently re-derived here rather than calling
/// into `cook-cache` — this file's whole point is to check the real binary
/// against ground truth computed a different way.
fn object_paths(cache_dir: &Path, key: &[u8; 32]) -> (PathBuf, PathBuf, PathBuf) {
    let hex = to_hex(key);
    let blob = cache_dir.join(&hex[..2]).join(&hex[2..]);
    let meta = blob.with_extension("meta.json");
    let provenance = blob.with_extension("provenance.json");
    (blob, meta, provenance)
}

fn assert_object_absent(cache_dir: &Path, key: &[u8; 32], label: &str) {
    let (blob, meta, provenance) = object_paths(cache_dir, key);
    assert!(!blob.exists(), "{label}: blob should have been evicted: {blob:?}");
    assert!(!meta.exists(), "{label}: .meta.json should have been evicted: {meta:?}");
    assert!(
        !provenance.exists(),
        "{label}: .provenance.json should have been evicted: {provenance:?}"
    );
}

fn assert_object_present(cache_dir: &Path, key: &[u8; 32], label: &str) {
    let (blob, meta, provenance) = object_paths(cache_dir, key);
    assert!(blob.exists(), "{label}: blob should have survived: {blob:?}");
    assert!(meta.exists(), "{label}: .meta.json should have survived: {meta:?}");
    assert!(
        provenance.exists(),
        "{label}: .provenance.json should have survived: {provenance:?}"
    );
}

/// Seed one CAS object directly on disk: blob with `size` bytes of filler
/// content, a `.meta.json` sidecar serialized from a real `ArtifactMeta`
/// (never hand-written), and an (unparsed by gc) `.provenance.json`
/// sidecar. The blob's mtime is stamped to `age_secs` in the past via the
/// stable `std::fs::FileTimes` API.
fn seed_object(
    cache_dir: &Path,
    counter: u32,
    size: u64,
    age_secs: u64,
    kind: Option<&str>,
    recipe_namespace: &str,
) -> [u8; 32] {
    let key = key_from_counter(counter);
    let hex = to_hex(&key);
    let shard_dir = cache_dir.join(&hex[..2]);
    fs::create_dir_all(&shard_dir).unwrap();
    let blob_path = shard_dir.join(&hex[2..]);
    fs::write(&blob_path, vec![b'x'; size as usize]).unwrap();

    let meta = ArtifactMeta {
        recipe_namespace: recipe_namespace.to_string(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 1,
        size_bytes: size,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: 0,
        output_path: format!("seeded-{counter}.bin"),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: kind.map(str::to_string),
        mode: ArtifactMeta::default_mode(),
        target: None,
    };
    fs::write(
        blob_path.with_extension("meta.json"),
        serde_json::to_vec(&meta).expect("serialize seeded ArtifactMeta"),
    )
    .unwrap();
    fs::write(blob_path.with_extension("provenance.json"), b"{}").unwrap();

    let mtime = SystemTime::now()
        .checked_sub(Duration::from_secs(age_secs))
        .expect("age_secs must not underflow SystemTime");
    let file = fs::File::open(&blob_path).expect("open seeded blob to stamp mtime");
    file.set_times(fs::FileTimes::new().set_modified(mtime))
        .expect("stamp seeded blob mtime");

    key
}

/// Recursively list every plain file under `dir`, as paths relative to
/// `dir`. Used to prove "the store is unchanged" by full-set comparison
/// rather than just a total-bytes check.
fn snapshot_files(dir: &Path) -> std::collections::BTreeSet<PathBuf> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().strip_prefix(dir).unwrap().to_path_buf())
        .collect()
}

/// Assert stdout's first line is exactly `Store: <cache_dir>`. `cache_dir`
/// is itself inside the fixture's own `TempDir`, so this single assertion
/// simultaneously proves "gc reported on the store we configured" and "gc
/// never touched anything outside this sandbox".
fn assert_store_line(text: &str, cache_dir: &Path) {
    let first = text.lines().next().expect("gc must print at least one line");
    assert_eq!(first, format!("Store: {}", cache_dir.display()));
}

/// Parse a `"Would free: N objects, SIZE"` or `"Freed: N objects, SIZE"`
/// line (always line 2 of a victims-non-empty report) into `(N, "SIZE")`.
fn parse_free_line(text: &str) -> (usize, String) {
    let line = text.lines().nth(1).expect("gc report must have a second line");
    let rest = line
        .strip_prefix("Would free: ")
        .or_else(|| line.strip_prefix("Freed: "))
        .unwrap_or_else(|| panic!("unexpected second line: {line:?} (full output:\n{text})"));
    let (n, size) = rest
        .split_once(" objects, ")
        .unwrap_or_else(|| panic!("could not parse free line: {line:?}"));
    (n.parse().expect("object count is a number"), size.to_string())
}

/// Parse the transition line's object counts out of `"Store: X -> Y (N
/// objects -> M)"` (always line 3 of a victims-non-empty report). Byte
/// totals on this line are deliberately NOT asserted anywhere in this file:
/// unlike the seeded per-object sizes, `total_before`/`total_after` are
/// sums the tests don't fully control the rounding of.
fn parse_transition_counts(text: &str) -> (usize, usize) {
    let line = text.lines().nth(2).expect("gc report must have a third line");
    let inside = line
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_else(|| panic!("could not parse transition line: {line:?}"));
    let (before, after) = inside
        .split_once(" objects -> ")
        .unwrap_or_else(|| panic!("could not parse object counts from: {inside:?}"));
    (
        before.trim().parse().expect("before-count is a number"),
        after.trim().parse().expect("after-count is a number"),
    )
}

// ---------------------------------------------------------------------------
// max_size: exact LRU tail, exempt kinds untouched
// ---------------------------------------------------------------------------

#[test]
fn max_size_evicts_exactly_the_lru_tail_of_files() {
    let fx = Fixture::new();

    // Five plain-file (kind = None) blobs, 1,000,000 bytes each, staggered
    // oldest-to-newest. Total file bytes: 5,000,000.
    let file_keys: Vec<[u8; 32]> = (0..5)
        .map(|i| fx.seed(i, 1_000_000, (5 - i) as u64 * 1000, None, "/Cookfile::build"))
        .collect();

    // One object of every size-sweep-exempt kind, each small and old — old
    // enough that a naive byte-hungry LRU (ignoring the exemption) would
    // take them before any file, and none of them touched since only
    // `--max-size` is passed (no `--older-than`, so the age pass never
    // runs at all).
    const EXEMPT_KINDS: &[&str] = &[
        "discovered_input_sets",
        "discovered_inputs",
        "probe_value",
        "symlink",
        "dir",
    ];
    let exempt_keys: Vec<[u8; 32]> = EXEMPT_KINDS
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            fx.seed(100 + i as u32, 10_000, 50_000, Some(kind), "/Cookfile::exempt")
        })
        .collect();

    // Non-vacuity: `cache du`'s `By kind:` breakdown must show `file` plus
    // all five exempt kinds — if a sidecar were mis-seeded, `enumerate`
    // would silently degrade it to `kind: None`, and this assertion catches
    // that before the eviction assertions below could pass vacuously.
    let kinds = fx.kinds_reported();
    assert!(kinds.contains("file"), "expected a 'file' kind row; got {kinds:?}");
    for kind in EXEMPT_KINDS {
        assert!(kinds.contains(*kind), "expected a {kind:?} kind row; got {kinds:?}");
    }

    // total_before = 5,000,000 (files) + 5 * 10,000 (exempt) = 5,050,000.
    // target = 2,050,000 evicts exactly the 3 oldest files:
    //   after evicting 3: running_total = 5,050,000 - 3,000,000 = 2,050,000 <= target (stop)
    //   before evicting the 3rd: running_total = 3,050,000 > target (continue)
    let gc = fx.run(&["cache", "gc", "--max-size", "2050000"]);
    assert!(gc.status.success(), "gc must exit 0: {gc:?}");
    let text = String::from_utf8(gc.stdout).unwrap();

    assert_store_line(&text, &fx.cache_dir);
    let (freed_objects, freed_size) = parse_free_line(&text);
    assert_eq!(freed_objects, 3, "expected exactly the 3 oldest files evicted:\n{text}");
    assert_eq!(freed_size, "3.0 MB", "3 * 1,000,000 bytes must render as an exact 3.0 MB:\n{text}");

    let (count_before, count_after) = parse_transition_counts(&text);
    assert_eq!(count_before, 10, "5 files + 5 exempt objects seeded:\n{text}");
    assert_eq!(count_after, 7, "10 - 3 evicted = 7 remaining:\n{text}");

    // The exact LRU tail: the 3 OLDEST files (index 0, 1, 2) are gone; the
    // 2 newest files (index 3, 4) survive.
    for (i, key) in file_keys.iter().enumerate() {
        if i < 3 {
            assert_object_absent(&fx.cache_dir, key, &format!("file[{i}] (oldest tail)"));
        } else {
            assert_object_present(&fx.cache_dir, key, &format!("file[{i}] (newest)"));
        }
    }

    // No exempt-kind object was touched, regardless of age.
    for (kind, key) in EXEMPT_KINDS.iter().zip(&exempt_keys) {
        assert_object_present(&fx.cache_dir, key, kind);
    }
}

// ---------------------------------------------------------------------------
// The strand case: a manifest gating recent artifacts survives the size sweep
// ---------------------------------------------------------------------------

#[test]
fn a_manifest_gating_recent_artifacts_survives_the_size_sweep() {
    let fx = Fixture::new();

    // The manifest: small (9,000 bytes) and the OLDEST object in the store
    // (100,000s old) — exactly the shape a naive byte-hungry LRU would take
    // first.
    let manifest_key = fx.seed(0, 9_000, 100_000, Some("discovered_input_sets"), "/Cookfile::cc");

    // Two large, RECENT file objects the manifest is the only route to the
    // full keys of (in the real depfile-unit shape this test models, not
    // literally reproduced here — the policy doesn't need it to be, since
    // `plan_eviction` decides per-object, not per-graph-reachability).
    let file_a = fx.seed(1, 500_000, 200, None, "/Cookfile::cc");
    let file_b = fx.seed(2, 500_000, 100, None, "/Cookfile::cc");

    let kinds = fx.kinds_reported();
    assert!(
        kinds.contains("discovered_input_sets"),
        "expected a 'discovered_input_sets' kind row; got {kinds:?}"
    );
    assert!(kinds.contains("file"), "expected a 'file' kind row; got {kinds:?}");

    // total_before = 9,000 + 500,000 + 500,000 = 1,009,000.
    // target = 950,000: evicting the older of the two files brings
    // running_total to 509,000 (<= target), so the sweep stops after ONE
    // file — the manifest (exempt) is never a candidate, so it is never
    // subtracted out of running_total no matter how low the target is set.
    let gc = fx.run(&["cache", "gc", "--max-size", "950000"]);
    assert!(gc.status.success(), "gc must exit 0: {gc:?}");
    let text = String::from_utf8(gc.stdout).unwrap();

    assert_store_line(&text, &fx.cache_dir);
    let (freed_objects, freed_size) = parse_free_line(&text);
    assert_eq!(freed_objects, 1, "expected exactly one file evicted:\n{text}");
    assert_eq!(freed_size, "500.0 kB", "one 500,000-byte file must render as exact 500.0 kB:\n{text}");

    // The manifest — blob, .meta.json, AND .provenance.json — survives
    // untouched, even though it was the single oldest object in the store.
    assert_object_present(&fx.cache_dir, &manifest_key, "discovered_input_sets manifest");

    // The older file was the one evicted; the newer file survives.
    assert_object_absent(&fx.cache_dir, &file_a, "older file");
    assert_object_present(&fx.cache_dir, &file_b, "newer file");
}

// ---------------------------------------------------------------------------
// older_than: every kind, including exempt ones
// ---------------------------------------------------------------------------

#[test]
fn older_than_applies_to_every_kind_including_exempt_ones() {
    let fx = Fixture::new();

    const KINDS: &[Option<&str>] = &[
        None, // plain file
        Some("discovered_input_sets"),
        Some("discovered_inputs"),
        Some("probe_value"),
        Some("symlink"),
        Some("dir"),
    ];

    // One "old" (20 days) and one "new" (1 day) object per kind, all the
    // same modest size (irrelevant here — only `--older-than` is passed,
    // so the size pass never runs). `--older-than 10d` cutoff sits
    // squarely between the two ages, with an order-of-magnitude margin on
    // both sides so test-execution latency can never blur the boundary.
    let mut old_keys = Vec::new();
    let mut new_keys = Vec::new();
    for (i, kind) in KINDS.iter().enumerate() {
        let old_key = fx.seed(i as u32, 100_000, 20 * 86_400, *kind, "/Cookfile::mixed");
        let new_key = fx.seed(100 + i as u32, 100_000, 86_400, *kind, "/Cookfile::mixed");
        old_keys.push((*kind, old_key));
        new_keys.push((*kind, new_key));
    }

    let kinds = fx.kinds_reported();
    assert!(kinds.contains("file"), "expected a 'file' kind row; got {kinds:?}");
    for kind in KINDS.iter().flatten() {
        assert!(kinds.contains(*kind), "expected a {kind:?} kind row; got {kinds:?}");
    }

    let gc = fx.run(&["cache", "gc", "--older-than", "10d"]);
    assert!(gc.status.success(), "gc must exit 0: {gc:?}");
    let text = String::from_utf8(gc.stdout).unwrap();

    assert_store_line(&text, &fx.cache_dir);
    let (freed_objects, freed_size) = parse_free_line(&text);
    assert_eq!(freed_objects, 6, "one old object per kind (6 kinds) must be evicted:\n{text}");
    assert_eq!(freed_size, "600.0 kB", "6 * 100,000 bytes must render as exact 600.0 kB:\n{text}");

    let (count_before, count_after) = parse_transition_counts(&text);
    assert_eq!(count_before, 12, "6 kinds * 2 ages = 12 seeded objects:\n{text}");
    assert_eq!(count_after, 6, "12 - 6 evicted = 6 remaining:\n{text}");

    // Every OLD object is gone, INCLUDING the exempt kinds — the age pass
    // does not consult the size-sweep exemption at all.
    for (kind, key) in &old_keys {
        assert_object_absent(&fx.cache_dir, key, &format!("old {kind:?}"));
    }
    // Every NEW object survives, exempt or not.
    for (kind, key) in &new_keys {
        assert_object_present(&fx.cache_dir, key, &format!("new {kind:?}"));
    }
}

// ---------------------------------------------------------------------------
// dry-run: frees nothing; its projection matches a real run
// ---------------------------------------------------------------------------

#[test]
fn dry_run_frees_nothing_and_its_projection_matches_a_real_run() {
    let fx = Fixture::new();

    // 4 file objects, 1,000,000 bytes each, staggered oldest-to-newest.
    for i in 0..4 {
        fx.seed(i, 1_000_000, (4 - i) as u64 * 1000, None, "/Cookfile::build");
    }

    let before_snapshot = snapshot_files(&fx.cache_dir);
    assert_eq!(before_snapshot.len(), 12, "4 objects * 3 files each (blob+meta+provenance)");

    // target = 2,000,000: evicts exactly the 2 oldest files (frees
    // 2,000,000, leaving running_total at 2,000,000 <= target).
    let dry = fx.run(&["cache", "gc", "--max-size", "2000000", "--dry-run"]);
    assert!(dry.status.success(), "dry-run gc must exit 0: {dry:?}");
    let dry_text = String::from_utf8(dry.stdout).unwrap();
    assert_store_line(&dry_text, &fx.cache_dir);

    // The store must be byte-for-byte unchanged: same file set.
    let after_dry_snapshot = snapshot_files(&fx.cache_dir);
    assert_eq!(
        before_snapshot, after_dry_snapshot,
        "a --dry-run must not delete or modify anything on disk"
    );

    let (projected_objects, projected_size) = parse_free_line(&dry_text);
    assert_eq!(projected_objects, 2, "expected the 2 oldest files projected:\n{dry_text}");
    assert_eq!(projected_size, "2.0 MB", "2 * 1,000,000 bytes must render as exact 2.0 MB:\n{dry_text}");

    // The identical command, without --dry-run: the real freed numbers
    // must equal the dry-run's projection exactly.
    let real = fx.run(&["cache", "gc", "--max-size", "2000000"]);
    assert!(real.status.success(), "real gc must exit 0: {real:?}");
    let real_text = String::from_utf8(real.stdout).unwrap();
    assert_store_line(&real_text, &fx.cache_dir);

    let (actual_objects, actual_size) = parse_free_line(&real_text);
    assert_eq!(
        actual_objects, projected_objects,
        "the real sweep's freed-object count must equal the dry-run's projection"
    );
    assert_eq!(
        actual_size, projected_size,
        "the real sweep's freed-byte figure must equal the dry-run's projection"
    );

    // And now the store genuinely changed: 2 objects' worth of files gone.
    let after_real_snapshot = snapshot_files(&fx.cache_dir);
    assert_eq!(after_real_snapshot.len(), 6, "2 objects evicted * 3 files each removed from 12");
}

// ---------------------------------------------------------------------------
// Usage error
// ---------------------------------------------------------------------------

#[test]
fn neither_flag_is_a_usage_error() {
    let fx = Fixture::new();
    let gc = fx.run(&["cache", "gc"]);
    assert!(!gc.status.success(), "neither --max-size nor --older-than must be a usage error: {gc:?}");
    let stderr = String::from_utf8_lossy(&gc.stderr);
    assert!(stderr.contains("--max-size"), "stderr must name --max-size; got: {stderr}");
    assert!(stderr.contains("--older-than"), "stderr must name --older-than; got: {stderr}");
}

// ---------------------------------------------------------------------------
// Missing store
// ---------------------------------------------------------------------------

#[test]
fn missing_store_is_a_no_op_and_is_not_created() {
    let fx = Fixture::new();
    assert!(!fx.cache_dir.exists(), "precondition: cache_dir must not exist yet");

    let gc = fx.run(&["cache", "gc", "--max-size", "10MB"]);
    assert!(gc.status.success(), "a missing store is a zero-work report, not an error: {gc:?}");
    let text = String::from_utf8(gc.stdout).unwrap();
    assert_eq!(
        text,
        format!("Store: {}\nNothing to evict.\n", fx.cache_dir.display()),
        "a missing store must print the exact two-line 'Nothing to evict' report"
    );
    assert!(
        !fx.cache_dir.exists(),
        "cook cache gc must never create the store directory as a side effect of inspecting it"
    );
}

// ---------------------------------------------------------------------------
// Cloud backend defers to the server
// ---------------------------------------------------------------------------

#[test]
fn cloud_backend_defers_to_the_server_and_deletes_nothing_locally() {
    let fx = Fixture::new();

    // Seed a small local store first: the cloud-enabled branch must return
    // before ever calling `resolved_cache_dir` / `LocalBackend::enumerate`,
    // so nothing seeded here should move.
    let key_a = fx.seed(0, 1_000_000, 50_000, None, "/Cookfile::build");
    let key_b = fx.seed(1, 1_000_000, 1000, Some("probe_value"), "/Cookfile::build");
    let before = snapshot_files(&fx.cache_dir);
    assert_eq!(before.len(), 6, "2 objects * 3 files each");

    fx.enable_cloud();

    // All three of endpoint, project, and an API key are required by
    // `CloudConfig::load_or_default` once `[cloud] enabled = true`.
    let gc = fx.run_with_env(
        &["cache", "gc", "--max-size", "1B"],
        &[("COOK_CLOUD_API_KEY", "dummy-test-key")],
    );
    assert!(gc.status.success(), "cloud-enabled gc must exit 0: {gc:?}");
    let text = String::from_utf8(gc.stdout).unwrap();
    assert!(
        text.contains("cloud cache is server-managed"),
        "expected the server-managed note; got: {text}"
    );

    // Every seeded file survives, byte-for-byte.
    let after = snapshot_files(&fx.cache_dir);
    assert_eq!(before, after, "the cloud-enabled path must delete nothing locally");
    assert_object_present(&fx.cache_dir, &key_a, "file object");
    assert_object_present(&fx.cache_dir, &key_b, "probe_value object");
}
