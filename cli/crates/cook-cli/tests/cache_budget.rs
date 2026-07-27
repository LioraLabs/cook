//! End-to-end proof of the end-of-run CAS store-budget check (COOK-235),
//! driving the real `cook` binary via `CARGO_BIN_EXE_cook` against real
//! builds that really publish to a real, on-disk CAS.
//!
//! Unit tests can (and do) cover the renderers and the config knobs. Nothing
//! below is provable there: every assertion here is about what a *build*
//! does — whether it warns, whether it deletes, and whether it walks the
//! store at all.
//!
//! ISOLATION IS NON-NEGOTIABLE — `auto_gc = true` deletes files, and the
//! developer's real CAS at `~/.cache/cook/cloud` holds real artifacts. The
//! pattern is `tests/cache_gc.rs`'s, verbatim:
//!
//!   * one [`Fixture`] type owning a `TempDir`, and every fixture writes its
//!     `.cook/cloud.toml` with an **absolute** `[cache] cache_dir` inside
//!     that `TempDir` before the first `cook` invocation
//!     (`CloudConfig::resolved_cache_dir` takes `cache_dir` verbatim —
//!     `PathBuf::from`, no join onto the project root, no canonicalize — so
//!     a relative path would resolve against whatever the engine's current
//!     directory happens to be, not the fixture);
//!   * a single `Command::new` construction site, [`Fixture::run_with_env`],
//!     which sets `XDG_CACHE_HOME` into the same `TempDir` **after** the
//!     caller's env loop, so a caller cannot override the second belt. There
//!     is no code path in this file that can construct a `Command` without
//!     the isolation.
//!
//! The shared fixture is a single fan-out recipe over `src/*.txt`, one
//! ~100 kB output per source. `Fixture::touch_sources(round)` rewrites every
//! source, so a round invalidates every unit and republishes the whole set —
//! `SOURCE_COUNT` fresh CAS objects of `OUTPUT_BYTES` each, ~800 kB per
//! round against a `max_size` an order of magnitude larger. That granularity
//! is deliberate and load-bearing for `auto_gc_true_self_caps_near_the_low_water_mark`:
//! `plan_eviction`'s size pass stops at the first candidate that brings the
//! running total to or below the low-water target, so the settled total can
//! undershoot by at most one object. Few large objects would undershoot the
//! 80% mark badly enough to make "settles near, not far below" unprovable.
//!
//! The fan-out fixture also publishes *only* plain-file artifacts: no probes,
//! no discovered-input manifests, so zero bytes of the size-sweep-exempt kinds
//! (`discovered_input_sets`, `discovered_inputs`, `probe_value`, `symlink`,
//! `dir`). Those are never evicted by the size pass, so an exempt-heavy
//! fixture could not reach the low-water mark at all.
//!
//! Test 6 is the deliberate mirror image, on [`EXEMPT_COOKFILE`]: a store whose
//! every byte IS an exempt kind, which is the only way to reach the sweep's
//! "planned no victims" arm. It gets its own Cookfile rather than a flag on the
//! shared one because the two fixtures want opposite things from every unit —
//! the fan-out one exists to publish evictable bytes, that one exists to
//! publish none.
//!
//! Assertions match stable substrings, never whole formatted lines: sizes
//! render through `cache_du::human_size`, which always emits one decimal
//! place in the kB/MB/GB bands (a `max_size = "2MB"` budget prints as
//! `2.0 MB` in the prose while the remediation command echoes the literal
//! verbatim as `--max-size 2MB`). Reproducing a whole line would make these
//! tests brittle against `human_size` rounding rather than against
//! behaviour.
//!
//! Known accepted gap, narrowed by COOK-359: the register-phase probe
//! pre-pass publishes probe values without incrementing the publish counter,
//! so a `published_count == 0` run can still add a handful of small objects.
//! The two silence tests below therefore assert the *absence of the check's
//! stderr lines*, never that the store is byte-for-byte unchanged.
//!
//! The other half of this note used to say the pre-pass wrote outside every
//! `publish_enabled` guard (COOK-339). That is fixed: the pre-pass evaluates
//! through `cook_probe::eval`, whose `record` honours `publish_enabled`, and
//! a `--no-publish` run now adds nothing to the store. It was also never
//! reachable before, because the pre-pass had no backend at all — which is
//! the defect COOK-359 was really about.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

use cook_engine::cook_cache::backend::ArtifactMeta;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// Sources in the fan-out fixture, and therefore CAS objects published per
/// round.
const SOURCE_COUNT: usize = 8;

/// Filler bytes each unit writes, before the round marker is appended. Must
/// stay in sync with the `head -c 100000` literal in [`COOKFILE`] — it is
/// only used for the arithmetic in comments and bounds below.
const OUTPUT_BYTES: u64 = 100_000;

/// Bytes one full round publishes, near enough for the bounds below (each
/// object is `OUTPUT_BYTES` plus a single-digit round marker).
const ROUND_BYTES: u64 = SOURCE_COUNT as u64 * OUTPUT_BYTES;

/// One fan-out `cook` step per `src/*.txt`, each writing ~100 kB of filler
/// plus its source's content — so rewriting the sources changes every
/// output's content hash and republishes the whole set.
///
/// `tr "\\0" "x"` is a Rust raw string: the shell sees `tr "\0" "x"` and
/// `tr` sees an escaped NUL, which is the byte `head -c … /dev/zero`
/// produces.
const COOKFILE: &str = r#"recipe build
    ingredients "src/*.txt"
    cook "out/$<in.stem>.bin" { head -c 100000 /dev/zero | tr "\\0" "x" > $<out>; cat $<in> >> $<out> }
"#;

/// Test 6's fixture: a project that can publish an all-exempt store, plus one
/// recipe that deliberately breaks that property as the test's control.
///
/// `check` is the load-bearing half, and both of its halves are chosen, not
/// incidental:
///
///   * `probe big` publishes its value with `kind: probe_value` — one of the
///     five `SIZE_SWEEP_EXEMPT_KINDS` — and bumps the run's publish counter at
///     that same put site (`executor.rs`, inside its `publish_enabled` guard).
///     `seq 1 1000` is ~4.9 kB of deterministic bytes: comfortably over the
///     1 kB budget test 6 configures, byte-identical run to run, and free of
///     any shell escaping the Cookfile parser would have to survive. It
///     declares `ingredients "seed.txt"` because CS-0178 gives a probe with no
///     declared inputs no cache key: such a probe skips the PUT alongside the
///     GET, so it would publish nothing, leave the store empty, and make both
///     of test 6's preconditions unsatisfiable. The declaration is what makes
///     the probe the store's one exempt object rather than no object.
///   * the consuming unit declares NO outputs, so it publishes no plain-file
///     artifact at all. It still bumps the publish counter — a publishing unit
///     writes at least its determinant manifest — which is what lets the run
///     trip the check's `published_count > 0` gate without putting a single
///     evictable byte in the store.
///
/// The probe is reached via `probes = {…}` on `cook.add_unit`, which is the
/// EXECUTE-phase probe path. That is load-bearing rather than stylistic:
/// `ingredients <probe>` fan-out would drive the register-phase pre-pass
/// instead, whose CAS writes sit outside the publish counter entirely
/// (COOK-339) — leaving `published_count == 0`, short-circuiting the check at
/// step 1, and testing nothing.
///
/// `file` is the control: one ~113 kB plain-file output, `kind: None`, the one
/// thing an all-exempt store lacks. Running it on the same fixture, with the
/// same config, is what proves `auto_gc = true` was genuinely live.
const EXEMPT_COOKFILE: &str = r#"probe big
    ingredients "seed.txt"
    { seq 1 1000 }

recipe check
    cook.add_unit({
        name    = "consume",
        inputs  = {},
        outputs = {},
        probes  = {"big"},
        command = "true",
    })

recipe file
    cook "out/big.bin" { seq 1 20000 > $<out> }
"#;

/// One isolated fixture: its own scratch `TempDir` containing a project
/// directory, a (not-yet-created) CAS `cache_dir`, and an `xdg_dir` used only
/// as the `XDG_CACHE_HOME` belt-and-suspenders. `_scratch` is kept alive for
/// the fixture's lifetime purely so its `Drop` doesn't clean the directory
/// out from under a running test; nothing reads it directly.
struct Fixture {
    _scratch: TempDir,
    project_dir: PathBuf,
    cache_dir: PathBuf,
    xdg_dir: PathBuf,
}

impl Fixture {
    /// A fan-out project wired to this fixture's own CAS, with `max_size`
    /// set and `auto_gc` written only when `auto_gc` is `Some` — `None`
    /// omits the key entirely, which is the shipped default the warn-only
    /// test pins.
    fn new(max_size: &str, auto_gc: Option<bool>) -> Self {
        Self::with_cookfile(COOKFILE, max_size, auto_gc)
    }

    /// The same fixture over an arbitrary Cookfile — test 6 needs a project
    /// that publishes only exempt kinds, which the fan-out [`COOKFILE`] is
    /// built to never be. Everything that makes a fixture safe is here and
    /// nowhere else: the absolute `[cache] cache_dir` inside this fixture's
    /// own `TempDir`, written before any `cook` invocation.
    fn with_cookfile(cookfile: &str, max_size: &str, auto_gc: Option<bool>) -> Self {
        let scratch = TempDir::new().unwrap();
        let project_dir = scratch.path().join("project");
        let cache_dir = scratch.path().join("cas");
        let xdg_dir = scratch.path().join("xdg");
        fs::create_dir_all(project_dir.join("src")).unwrap();
        fs::write(project_dir.join("Cookfile"), cookfile).unwrap();
        write_cloud_toml(&project_dir, &cache_dir, max_size, auto_gc);
        Self {
            _scratch: scratch,
            project_dir,
            cache_dir,
            xdg_dir,
        }
    }

    /// Rewrite every source with `round`'s marker. Round 1 also creates
    /// them; the `ingredients "src/*.txt"` glob is resolved at registration,
    /// so this must run before the first invocation. Keep `round` a single
    /// digit so every object is exactly the same size.
    fn touch_sources(&self, round: u32) {
        assert!((1..=9).contains(&round), "round must stay a single digit");
        for i in 0..SOURCE_COUNT {
            fs::write(
                self.project_dir.join(format!("src/f{i}.txt")),
                format!("round {round}\n"),
            )
            .unwrap();
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }

    /// The ONLY `Command` construction site in this file. `XDG_CACHE_HOME` is
    /// set last, after the caller's own envs, so a caller-supplied value can
    /// never override this belt: this file's subject deletes files, and the
    /// developer's real CAS holds real artifacts.
    fn run_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(cook_bin());
        cmd.args(args).current_dir(&self.project_dir);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.env("XDG_CACHE_HOME", &self.xdg_dir);
        cmd.output().expect("run cook")
    }

    /// Rewrite the sources for `round`, build, and assert the build itself
    /// succeeded. Returns the run's stderr.
    fn build_round(&self, round: u32) -> String {
        self.touch_sources(round);
        let out = self.run(&["build"]);
        assert!(out.status.success(), "round {round} build failed: {out:?}");
        stderr_of(&out)
    }

    /// Independently walk this fixture's own `cache_dir` for exactly the
    /// blobs `LocalBackend::enumerate` counts: files whose name is 62
    /// lowercase hex characters. Deliberately not `cook cache du` — an
    /// assertion about the store must not depend on another command's output
    /// formatting.
    fn blobs(&self) -> Vec<PathBuf> {
        let mut blobs = Vec::new();
        for entry in walkdir::WalkDir::new(&self.cache_dir).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            let is_blob = name.len() == 62
                && name
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if is_blob {
                blobs.push(entry.path().to_path_buf());
            }
        }
        blobs
    }

    fn store(&self) -> Store {
        let mut store = Store {
            objects: 0,
            bytes: 0,
        };
        for blob in self.blobs() {
            store.objects += 1;
            store.bytes += fs::metadata(&blob).expect("stat blob").len();
        }
        store
    }

    /// Blob count bucketed by the `kind` each blob's `.meta.json` sidecar
    /// declares, deserialized through the real [`ArtifactMeta`] rather than
    /// grepped out of the JSON — a mis-read sidecar must fail loudly here,
    /// not silently degrade to `kind: None` and turn an exempt object into
    /// an apparently evictable one.
    ///
    /// `None` buckets under `"file"`, `cook cache du`'s display label for a
    /// plain-file artifact (there is no literal `"file"` kind string in
    /// production code) — and the one bucket that is NOT exempt from the size
    /// sweep. Test 6 asserts on the whole map rather than on a count, so an
    /// unexpected non-exempt object fails it instead of slipping past.
    fn kinds(&self) -> BTreeMap<String, usize> {
        let mut by_kind = BTreeMap::new();
        for blob in self.blobs() {
            let sidecar = blob.with_extension("meta.json");
            let bytes =
                fs::read(&sidecar).unwrap_or_else(|e| panic!("read {}: {e}", sidecar.display()));
            let meta: ArtifactMeta = serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("parse {}: {e}", sidecar.display()));
            *by_kind
                .entry(meta.kind.unwrap_or_else(|| "file".to_string()))
                .or_insert(0) += 1;
        }
        by_kind
    }
}

/// A store measurement: blob count and blob bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Store {
    objects: usize,
    bytes: u64,
}

/// Write `.cook/cloud.toml`: an absolute, verbatim `[cache] cache_dir` inside
/// the fixture's own `TempDir` (the isolation boundary), the budget literal,
/// and `auto_gc` only when the caller asked for it.
fn write_cloud_toml(project_dir: &Path, cache_dir: &Path, max_size: &str, auto_gc: Option<bool>) {
    fs::create_dir_all(project_dir.join(".cook")).unwrap();
    let mut toml = format!(
        "[cache]\ncache_dir = {:?}\nmax_size = {:?}\n",
        cache_dir.to_string_lossy(),
        max_size
    );
    if let Some(enabled) = auto_gc {
        toml.push_str(&format!("auto_gc = {enabled}\n"));
    }
    fs::write(project_dir.join(".cook/cloud.toml"), toml).unwrap();
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Every stderr line the budget check can emit, in order. Its four outputs —
/// the warning block's first line, the sweep report, and the two
/// degradation paths — are the only cook lines mentioning "cache store",
/// "cache budget check failed", or "cache gc failed". Collecting them rather
/// than grepping for one shape means an unexpected variant (a gc failure, a
/// walk failure) fails the silence tests instead of slipping past them.
fn budget_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| {
            l.contains("cache store")
                || l.contains("cache budget check failed")
                || l.contains("cache gc failed")
        })
        .collect()
}

fn warned(stderr: &str) -> bool {
    stderr.contains("cook: warning: cache store is")
}

fn swept(stderr: &str) -> bool {
    stderr.contains("cook: cache store was")
}

/// Assert the full three-line warning block: the over-budget headline, the
/// paste-able remediation command echoing `max_size_literal` verbatim, and
/// the largest-namespace attribution (every fixture here has a namespace, so
/// the optional third line is always present — `largest` names which one).
fn assert_warning_block(stderr: &str, max_size_literal: &str, largest: &str) {
    assert!(
        stderr.contains("cook: warning: cache store is"),
        "expected the over-budget headline; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("over the"),
        "headline must name the budget it is over; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("budget ("),
        "headline must carry the percentage; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("cook cache gc --max-size {max_size_literal}")),
        "remediation must echo the configured literal {max_size_literal:?} verbatim; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("cook:          largest: {largest}")),
        "expected the largest-namespace attribution for {largest:?}; stderr:\n{stderr}"
    );
}

fn assert_silent(stderr: &str, label: &str) {
    let lines = budget_lines(stderr);
    assert!(
        lines.is_empty(),
        "{label}: the budget check must say nothing; got {lines:?}\nfull stderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 1. Warn-only is the default
// ---------------------------------------------------------------------------

/// One four-round warn-only scenario against a 2 MB budget, run twice: once
/// with `auto_gc` absent from `[cache]` entirely, once with an explicit
/// `auto_gc = false`. Returns per-round `(warned, Store)`.
///
/// Rounds 1 and 2 land under the budget (0.8 MB, 1.6 MB); rounds 3 and 4 are
/// over it (2.4 MB, 3.2 MB). The under-budget rounds are not filler: they are
/// what proves the warning is a consequence of exceeding the budget rather
/// than something every publishing run prints.
fn warn_only_rounds(auto_gc: Option<bool>) -> Vec<(bool, Store)> {
    let fx = Fixture::new("2MB", auto_gc);
    let mut observed = Vec::new();
    for round in 1..=4 {
        let stderr = fx.build_round(round);
        assert!(
            !swept(&stderr),
            "round {round}: warn-only must never sweep (auto_gc = {auto_gc:?}); stderr:\n{stderr}"
        );
        if round >= 3 {
            assert_warning_block(&stderr, "2MB", "/Cookfile::build");
        }
        observed.push((warned(&stderr), fx.store()));
    }
    observed
}

#[test]
fn warn_only_is_the_default_and_never_shrinks_the_store() {
    let absent = warn_only_rounds(None);
    let explicit_false = warn_only_rounds(Some(false));

    for (label, observed) in [("auto_gc absent", &absent), ("auto_gc = false", &explicit_false)] {
        let warnings: Vec<bool> = observed.iter().map(|(w, _)| *w).collect();
        assert_eq!(
            warnings,
            vec![false, false, true, true],
            "{label}: the two under-budget rounds must be silent and the two over-budget \
             rounds must warn; stores: {:?}",
            observed.iter().map(|(_, s)| *s).collect::<Vec<_>>()
        );

        // Every run published and the store only ever grew: warn-only must
        // not reduce the object count or the byte total across the runs.
        for pair in observed.windows(2) {
            let (before, after) = (pair[0].1, pair[1].1);
            assert!(
                after.objects > before.objects && after.bytes > before.bytes,
                "{label}: warn-only must never reduce the store; {before:?} -> {after:?}"
            );
        }
        assert_eq!(
            observed[3].1.objects,
            4 * SOURCE_COUNT,
            "{label}: four rounds must leave four rounds' worth of objects"
        );
    }

    // ... and "absent" is not merely similar to "= false", it is identical:
    // same object counts, same byte totals, same warning pattern, round for
    // round.
    assert_eq!(
        absent, explicit_false,
        "an absent auto_gc key must behave exactly like auto_gc = false"
    );
}

// ---------------------------------------------------------------------------
// 2. auto_gc = true self-caps
// ---------------------------------------------------------------------------

#[test]
fn auto_gc_true_self_caps_near_the_low_water_mark() {
    const BUDGET: u64 = 2_000_000;
    // `EvictPolicy::auto`'s DEFAULT_LOW_WATER is 0.8.
    const LOW_WATER: u64 = BUDGET / 10 * 8;

    let fx = Fixture::new("2MB", Some(true));

    let mut sweeps = 0;
    for round in 1..=6 {
        let stderr = fx.build_round(round);
        let store = fx.store();

        if !swept(&stderr) {
            // Only the first two rounds may go unswept: 0.8 MB and 1.6 MB
            // are genuinely under the 2 MB budget.
            assert!(
                round <= 2 && store.bytes <= BUDGET,
                "round {round}: an over-budget run with auto_gc = true must sweep; \
                 store {store:?}; stderr:\n{stderr}"
            );
            assert!(
                !warned(&stderr),
                "round {round}: an under-budget run must not warn either; stderr:\n{stderr}"
            );
            continue;
        }

        sweeps += 1;
        assert!(
            stderr.contains("cook: cache store was")
                && stderr.contains("over the")
                && stderr.contains("swept")
                && stderr.contains("now"),
            "round {round}: expected the one-line sweep report; stderr:\n{stderr}"
        );
        // A sweep replaces the warning; it does not accompany it.
        assert!(
            !warned(&stderr),
            "round {round}: a successful sweep must not also warn; stderr:\n{stderr}"
        );

        // The headline claim: after a run that swept, the store is inside
        // its budget.
        assert!(
            store.bytes <= BUDGET,
            "round {round}: a swept store must be at or under its {BUDGET}-byte budget; {store:?}"
        );
        // And it settled NEAR the low-water mark, not far below it. The size
        // pass stops at the first candidate that brings the running total to
        // or below the target, so the floor is `LOW_WATER - OUTPUT_BYTES`
        // (1,499,992 here, observed 1,500,120). The bound below leaves room
        // for that undershoot and nothing like enough for a sweep that took
        // the store to zero.
        assert!(
            store.bytes >= LOW_WATER - OUTPUT_BYTES,
            "round {round}: the sweep must settle near the {LOW_WATER}-byte low-water mark, \
             not far below it; {store:?}"
        );
    }

    // Rounds 3..=6 all publish a full round on top of a store already at the
    // low-water mark, so every one of them must have swept.
    assert_eq!(sweeps, 4, "expected rounds 3..=6 to each trigger a sweep");

    // Non-vacuity against test 1's fixture: six warn-only rounds would have
    // left 4.8 MB in the store. Self-capping is a real difference, not a
    // fixture that never grew.
    let store = fx.store();
    assert!(
        store.bytes < 6 * ROUND_BYTES,
        "auto_gc must have actually reclaimed bytes; {store:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. A settled no-op build neither warns nor walks
// ---------------------------------------------------------------------------

#[test]
fn a_settled_no_op_build_neither_warns_nor_walks_the_store() {
    // Absurdly small on purpose. This is what makes the test prove the WALK
    // was skipped rather than merely that the store happened to be under
    // budget: any walk at all would find a ~800 kB store 800x over a 1 kB
    // budget and warn. Silence on the second run is only possible if the
    // `published_count == 0` short-circuit fired before the config load and
    // before the walk — the ordering COOK-313's optimized path depends on.
    let fx = Fixture::new("1kB", None);

    let first = fx.build_round(1);
    assert_warning_block(&first, "1kB", "/Cookfile::build");
    let after_first = fx.store();
    assert!(
        after_first.bytes > 1_000,
        "precondition: the store must be genuinely over its 1 kB budget; {after_first:?}"
    );

    // Identical build, nothing changed: every unit is a cache hit, nothing is
    // published, and the check must not run at all.
    let second = fx.run(&["build"]);
    assert!(second.status.success(), "settled rebuild failed: {second:?}");
    let stderr = stderr_of(&second);
    assert_silent(&stderr, "a settled no-op build");
}

// ---------------------------------------------------------------------------
// 4. --no-auto-gc disables the sweep
// ---------------------------------------------------------------------------

#[test]
fn no_auto_gc_disables_the_sweep_but_keeps_the_warning() {
    let fx = Fixture::new("2MB", Some(true));

    // Seed to 1.6 MB — under the 2 MB budget, so neither round says anything
    // and the store is one round away from over-budget.
    for round in 1..=2 {
        let stderr = fx.build_round(round);
        assert_silent(&stderr, &format!("under-budget seeding round {round}"));
    }

    // The same suppression reached three ways. The third is `partition_argv`'s
    // path: a global flag positioned AFTER the recipe name.
    let variants: [(&str, &[&str], &[(&str, &str)]); 3] = [
        ("--no-auto-gc before the recipe", &["--no-auto-gc", "build"], &[]),
        ("COOK_NO_AUTO_GC=1", &["build"], &[("COOK_NO_AUTO_GC", "1")]),
        ("--no-auto-gc after the recipe", &["build", "--no-auto-gc"], &[]),
    ];

    for (round, (label, args, envs)) in (3u32..).zip(variants) {
        let before = fx.store();
        fx.touch_sources(round);
        let out = fx.run_with_env(args, envs);
        assert!(out.status.success(), "{label}: build failed: {out:?}");
        let stderr = stderr_of(&out);

        assert_warning_block(&stderr, "2MB", "/Cookfile::build");
        assert!(
            !swept(&stderr),
            "{label}: the sweep must be suppressed; stderr:\n{stderr}"
        );

        // Not reduced — in fact grown by exactly one round's publishes.
        let after = fx.store();
        assert_eq!(
            after.objects,
            before.objects + SOURCE_COUNT,
            "{label}: nothing may be evicted; {before:?} -> {after:?}"
        );
        assert!(
            after.bytes > before.bytes,
            "{label}: the store must not shrink; {before:?} -> {after:?}"
        );
    }

    // The control that makes the three assertions above discriminating: the
    // identical sixth round WITHOUT any suppression sweeps, on the same
    // fixture, from the same over-budget state. Without this, "no sweep
    // happened" would also be satisfied by a fixture that never sweeps.
    let before = fx.store();
    let stderr = fx.build_round(6);
    assert!(
        swept(&stderr),
        "control: the same over-budget fixture must sweep with auto_gc on and no override; \
         stderr:\n{stderr}"
    );
    let after = fx.store();
    assert!(
        after.bytes < before.bytes && after.objects < before.objects,
        "control: the unsuppressed sweep must actually reclaim; {before:?} -> {after:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. --no-publish never triggers the check
// ---------------------------------------------------------------------------

#[test]
fn no_publish_never_triggers_the_check_even_over_budget() {
    // Both halves of this fixture are load-bearing, and the test is vacuous
    // without either:
    //
    //   * the store must already be OVER budget before the --no-publish run,
    //     or `total <= budget` would keep the run silent whether or not the
    //     publish gate works. A tiny 1 kB budget plus one real publishing
    //     round gets there, and the round's own warning proves it;
    //   * the --no-publish run must actually EXECUTE work, or it publishes
    //     nothing for the ordinary reason and the test degenerates into
    //     `a_settled_no_op_build_neither_warns_nor_walks_the_store`.
    //
    // With both in place: correct code leaves `published_count == 0`, the
    // check short-circuits, and the run is silent. A publish counter bumped
    // outside a `publish_enabled` guard would make the count non-zero, walk
    // the already-over-budget store, and warn — failing this test.
    let fx = Fixture::new("1kB", None);

    let first = fx.build_round(1);
    assert_warning_block(&first, "1kB", "/Cookfile::build");

    let before = fx.store();
    assert!(
        before.bytes > 1_000,
        "precondition: the store must already be over its 1 kB budget before the \
         --no-publish run; {before:?}"
    );

    // Change every source, so the --no-publish run really re-executes every
    // unit rather than replaying cache hits.
    fx.touch_sources(2);
    let out = fx.run(&["--no-publish", "build"]);
    assert!(out.status.success(), "--no-publish build failed: {out:?}");
    let stderr = stderr_of(&out);

    // Work really happened: every output on disk carries round 2's marker,
    // which only the re-executed command could have written.
    for i in 0..SOURCE_COUNT {
        let produced = fs::read_to_string(fx.project_dir.join(format!("out/f{i}.bin"))).unwrap();
        assert!(
            produced.ends_with("round 2\n"),
            "out/f{i}.bin must carry round 2's marker — the --no-publish run has to have \
             executed real work for this test to discriminate"
        );
    }

    assert_silent(&stderr, "a --no-publish build");

    // And it stayed over budget throughout, so the silence above cannot be
    // explained by the store having fallen under the budget. (Byte-for-byte
    // equality is deliberately NOT asserted: COOK-339's pre-pass probe writes
    // sit outside the publish counter.)
    let after = fx.store();
    assert!(
        after.bytes > 1_000,
        "the store must still be over its 1 kB budget after the silent run; {after:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. An all-exempt over-budget store warns; it does not go silent
// ---------------------------------------------------------------------------

/// The `SIZE_SWEEP_EXEMPT_KINDS` floor, end to end. `plan_eviction` chooses no
/// victims over an all-exempt candidate set however far over budget it is, so
/// `auto_gc = true` sweeps nothing — and the check must fall through to the
/// warning rather than fall quiet. Deleting that fall-through (making the
/// no-victims arm `return`) would leave such a store over budget forever with
/// no signal at all, and this is the only test at any level that would notice.
///
/// The two preconditions below are the test, as much as the assertions are.
/// Without the first the check never runs; without the second the sweep finds a
/// victim, takes the `outcome: Some(_)` arm, and the run proves test 2's claim
/// over again instead of this one. See [`EXEMPT_COOKFILE`] for why a probe plus
/// an output-less unit is what satisfies both at once.
#[test]
fn auto_gc_true_warns_when_the_sweep_can_only_find_exempt_kinds() {
    let fx = Fixture::with_cookfile(EXEMPT_COOKFILE, "1kB", Some(true));

    // `probe big`'s declared input. Written outside `src/`, so it stays clear of
    // the fan-out fixture's `ingredients "src/*.txt"` glob and its
    // `SOURCE_COUNT` accounting. It must exist before the first invocation
    // because a probe's `ingredients` glob resolves at registration.
    fs::write(fx.project_dir.join("seed.txt"), "seed\n").unwrap();

    let out = fx.run(&["check"]);
    assert!(out.status.success(), "probe-only build failed: {out:?}");
    let stderr = stderr_of(&out);

    // Precondition 1: the store really is over its budget, so the check
    // reached step 7 rather than returning at `total <= budget`.
    let before = fx.store();
    assert!(
        before.bytes > 1_000,
        "precondition: the store must be genuinely over its 1 kB budget; {before:?}"
    );

    // Precondition 2, the one this test rests on: every object in the store is
    // a size-sweep-exempt kind, so `plan_eviction` has nothing it is permitted
    // to evict. Asserted as the whole by-kind map, not as "no `file` objects":
    // any unexpected kind at all — a discovered-inputs manifest, a stray
    // directory marker — has to fail here rather than quietly change which arm
    // the run takes.
    assert_eq!(
        fx.kinds(),
        BTreeMap::from([("probe_value".to_string(), 1usize)]),
        "precondition: the store must hold exactly one object, of an exempt kind; \
         a non-exempt object would give the sweep a victim and test the wrong arm"
    );

    // The claim: over budget, `auto_gc` on, the sweep planned no victims — and
    // the check warned anyway.
    assert_warning_block(&stderr, "1kB", "probe:big");
    assert!(
        !swept(&stderr),
        "an all-exempt store has nothing to sweep, so no sweep report may be printed; \
         stderr:\n{stderr}"
    );
    assert_eq!(
        fx.store(),
        before,
        "a sweep that planned no victims must not have removed anything"
    );

    // Non-vacuity: `auto_gc = true` was genuinely live for the run above.
    // Same fixture, same config, same over-budget store — the only change is
    // one non-exempt object, and now it sweeps. Without this control, "warned
    // instead of sweeping" would be equally satisfied by an `auto_gc` that was
    // never switched on, and the warning above would prove nothing about the
    // sweep's no-victims arm.
    let out = fx.run(&["file"]);
    assert!(out.status.success(), "control build failed: {out:?}");
    let stderr = stderr_of(&out);
    assert!(
        swept(&stderr),
        "control: the same fixture must sweep once the store holds a non-exempt \
         object; stderr:\n{stderr}"
    );
    assert!(
        !warned(&stderr),
        "control: a successful sweep must not also warn; stderr:\n{stderr}"
    );

    // And the exemption is what it says it is: the sweep that reclaimed the
    // control's plain file left the probe value exactly where it was.
    assert_eq!(
        fx.kinds(),
        BTreeMap::from([("probe_value".to_string(), 1usize)]),
        "the sweep must have evicted the non-exempt object and spared the exempt one"
    );
    assert_eq!(
        fx.store(),
        before,
        "the swept store must be back to precisely its all-exempt contents"
    );
}
