//! End-to-end tests for `cook cache du` (COOK-232), driving the real `cook`
//! binary via `CARGO_BIN_EXE_cook` against a real, on-disk seeded CAS.
//!
//! ISOLATION IS NON-NEGOTIABLE: every fixture here writes its own
//! `.cook/cloud.toml` with `[cache] cache_dir` pointing at an absolute path
//! inside the test's own `TempDir` *before* the first `cook` invocation.
//! `CloudConfig::resolved_cache_dir` takes a configured `cache_dir` verbatim
//! (`PathBuf::from`, no join onto the project root, no canonicalize), so an
//! absolute path is required — a relative one would resolve against
//! whatever the engine's current directory happens to be, not the fixture.
//! None of these tests may read or write the developer's real CAS at
//! `~/.cache/cook/cloud`.
//!
//! Coverage:
//!   * `seeded_store_total_matches_an_independent_stat_walk` — the headline
//!     cross-check: `du`'s reported byte/object total against a plain
//!     `walkdir` scan of the same directory, counting only 62-hex blob
//!     files (never the `.meta.json` / `.provenance.json` / `.tmp`
//!     sidecars).
//!   * `by_kind_and_by_namespace_breakdown_is_accurate_and_unprefixed` —
//!     the `By kind:` counts sum to the total, both recipes' namespace
//!     strings appear verbatim (no invented project prefix — COOK-333's
//!     empty `project_id` is a known, out-of-scope gap, not something `du`
//!     should paper over).
//!   * `budget_line_reflects_max_size_relative_to_the_seeded_store` — one
//!     seeded store, `du` rerun with a generous and then a minuscule
//!     `[cache] max_size`; both exit 0 (a budget is advisory).
//!   * `empty_store_reports_zero_total_without_error` — a `cache_dir` that
//!     doesn't exist on disk is a zero-total report, not an error.
//!   * `unparseable_max_size_names_the_offending_literal` — a bad
//!     `max_size` literal is a non-zero exit whose stderr quotes it.
//!   * `cloud_backend_defers_to_the_server_and_enumerates_nothing_locally`
//!     — `[cloud] enabled = true` short-circuits before any local walk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run cook")
}

fn run_with_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(cook_bin());
    cmd.args(args).current_dir(dir);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run cook")
}

/// Write `.cook/cloud.toml` with `[cache] cache_dir = <cache_dir>` (verbatim,
/// absolute) plus an optional `max_size` literal. This is the isolation
/// boundary every fixture in this file goes through — nothing here ever
/// falls back to `dirs::cache_dir()/cook/cloud`.
fn write_local_cloud_toml(project_dir: &Path, cache_dir: &Path, max_size: Option<&str>) {
    fs::create_dir_all(project_dir.join(".cook")).unwrap();
    let mut toml = format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy());
    if let Some(m) = max_size {
        toml.push_str(&format!("max_size = {m:?}\n"));
    }
    fs::write(project_dir.join(".cook/cloud.toml"), toml).unwrap();
}

/// A two-recipe fixture producing a mix of artifact kinds under two
/// distinct `recipe_namespace` values:
///   * `build` — two plain-file outputs (`kind = None`, folded into `du`'s
///     `"file"` row), namespace `/Cookfile::build`.
///   * `other` — a demand-driven `cook.probe` + `cook.add_unit` pair (the
///     same shape as `tests/probe.rs`'s
///     `probe_consumer_end_to_end_first_run_then_cache_hit`), which persists
///     a `probe_value` artifact under `probe:ns:greet` AND a plain-file
///     output under namespace `/Cookfile::other`.
///
/// The probe key is `ns:greet`, not a bare `greet`: empirically, a bare
/// identifier doesn't route through the `$<key.field>` sigil-substitution
/// grammar inside a `command` string — the reference is left as a literal
/// `$<greet.word>` and the shell rejects it as a syntax error. A
/// colon-namespaced key (matching every other `cook.probe` example in this
/// crate) substitutes correctly.
///
/// Isolation: `.cook/cloud.toml` is written before anything else, pointing
/// `[cache] cache_dir` at `cache_dir` (expected to be an absolute path
/// inside the caller's own `TempDir`).
fn seeded_project(cache_dir: &Path) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_local_cloud_toml(dir.path(), cache_dir, None);
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.txt"), "hello\n").unwrap();
    fs::write(dir.path().join("src/b.txt"), "world\n").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe build
    ingredients "src/*.txt"
    cook "out/$<in.stem>.up" { tr 'a-z' 'A-Z' < $<in> > $<out> }

recipe other
        cook.probe("ns:greet", {
            inputs = {},
            produce = "return { word = \"hi\" }",
        })
        cook.add_unit({
            name = "echo",
            inputs = {},
            outputs = {"other-out.txt"},
            probes = {"ns:greet"},
            command = "echo $<ns:greet.word> > other-out.txt",
        })
"#,
    )
    .unwrap();
    dir
}

/// Independently stat-walk `cache_dir`, counting exactly the blobs
/// `LocalBackend::enumerate` counts: files whose name is 62 lowercase hex
/// characters, summing `fs::metadata().len()`. This is a plain directory
/// scan wholly separate from `enumerate`'s own logic — the genuine
/// cross-check the task wants, not a restatement of the implementation.
/// `.meta.json`, `.provenance.json`, and `.tmp` sidecars are all excluded
/// because none of them are exactly 62 lowercase-hex characters.
fn stat_walk_store(cache_dir: &Path) -> (u64, usize) {
    let mut total_bytes: u64 = 0;
    let mut count: usize = 0;
    for entry in walkdir::WalkDir::new(cache_dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let is_blob = name.len() == 62
            && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !is_blob {
            continue;
        }
        count += 1;
        total_bytes += entry.metadata().expect("stat blob").len();
    }
    (total_bytes, count)
}

/// Parse `du`'s `Total: <human> (<N> bytes) across <M> objects` line into
/// `(N, M)`.
fn parse_total_line(text: &str) -> (u64, usize) {
    let line = text
        .lines()
        .find(|l| l.starts_with("Total: "))
        .unwrap_or_else(|| panic!("no Total: line in output:\n{text}"));
    let bytes_str = line
        .split('(')
        .nth(1)
        .and_then(|s| s.split(" bytes)").next())
        .unwrap_or_else(|| panic!("could not parse byte count from: {line}"));
    let count_str = line
        .split("across ")
        .nth(1)
        .and_then(|s| s.split(" objects").next())
        .unwrap_or_else(|| panic!("could not parse object count from: {line}"));
    (
        bytes_str.trim().parse().expect("byte count is a number"),
        count_str.trim().parse().expect("object count is a number"),
    )
}

/// Parse the `By kind:` section into `(label, object_count)` pairs.
fn parse_kind_counts(text: &str) -> Vec<(String, usize)> {
    parse_indented_section(text, "By kind:")
}

/// Parse the `By namespace:` section into `(label, object_count)` pairs.
/// The `… and N more` overflow line (irrelevant here — well under the
/// 20-row cap) is skipped since it has no `: ` separator to split on.
fn parse_namespace_counts(text: &str) -> Vec<(String, usize)> {
    parse_indented_section(text, "By namespace:")
}

fn parse_indented_section(text: &str, header: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        if line == header {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.is_empty() {
            break;
        }
        let trimmed = line.trim_start();
        let Some((label, rest)) = trimmed.split_once(": ") else {
            continue; // e.g. the "… and N more" overflow line
        };
        let Some(count_str) = rest.split(" objects").next() else {
            continue;
        };
        let Ok(count) = count_str.trim().parse::<usize>() else {
            continue;
        };
        out.push((label.to_string(), count));
    }
    out
}

// ---------------------------------------------------------------------------
// Seeded store totals
// ---------------------------------------------------------------------------

#[test]
fn seeded_store_total_matches_an_independent_stat_walk() {
    let scratch = TempDir::new().unwrap();
    let cache_dir = scratch.path().join("cas");
    let project = seeded_project(&cache_dir);

    let build = run(project.path(), &["build"]);
    assert!(build.status.success(), "build failed: {build:?}");
    let other = run(project.path(), &["other"]);
    assert!(other.status.success(), "other failed: {other:?}");

    let du = run(project.path(), &["cache", "du"]);
    assert!(du.status.success(), "du failed: {du:?}");
    let text = String::from_utf8(du.stdout).unwrap();

    let (reported_bytes, reported_count) = parse_total_line(&text);
    let (walked_bytes, walked_count) = stat_walk_store(&cache_dir);

    assert_eq!(
        reported_count, walked_count,
        "du's object count must match an independent stat-walk of the store;\ndu output:\n{text}"
    );
    assert_eq!(
        reported_bytes, walked_bytes,
        "du's byte total must match an independent stat-walk of the store;\ndu output:\n{text}"
    );
    // Sanity: the store isn't trivially empty (that would make the equality
    // above vacuous).
    assert!(walked_count >= 3, "expected at least 3 blobs seeded; got {walked_count}");
}

// ---------------------------------------------------------------------------
// Per-kind / per-namespace breakdown
// ---------------------------------------------------------------------------

#[test]
fn by_kind_and_by_namespace_breakdown_is_accurate_and_unprefixed() {
    let scratch = TempDir::new().unwrap();
    let cache_dir = scratch.path().join("cas");
    let project = seeded_project(&cache_dir);

    assert!(run(project.path(), &["build"]).status.success());
    assert!(run(project.path(), &["other"]).status.success());

    let du = run(project.path(), &["cache", "du"]);
    assert!(du.status.success());
    let text = String::from_utf8(du.stdout).unwrap();

    assert!(text.contains("By kind:\n"), "missing By kind: section:\n{text}");
    assert!(text.contains("By namespace:\n"), "missing By namespace: section:\n{text}");

    let (_, total_count) = parse_total_line(&text);

    let kinds = parse_kind_counts(&text);
    assert!(!kinds.is_empty(), "expected at least one kind row:\n{text}");
    let kind_sum: usize = kinds.iter().map(|(_, c)| c).sum();
    assert_eq!(
        kind_sum, total_count,
        "per-kind object counts must sum to the reported total; kinds={kinds:?}, total={total_count}\n{text}"
    );
    // A genuine mix: the plain-file outputs plus the probe_value artifact.
    assert!(
        kinds.iter().any(|(label, _)| label == "file"),
        "expected a 'file' kind row:\n{text}"
    );
    assert!(
        kinds.iter().any(|(label, _)| label == "probe_value"),
        "expected a 'probe_value' kind row (the non-file kind mixed into the seeded store):\n{text}"
    );

    let namespaces = parse_namespace_counts(&text);
    let ns_sum: usize = namespaces.iter().map(|(_, c)| c).sum();
    assert_eq!(
        ns_sum, total_count,
        "per-namespace object counts must also sum to the reported total; namespaces={namespaces:?}\n{text}"
    );

    // Both recipes' namespaces must appear VERBATIM: exactly
    // "/Cookfile::build" and "/Cookfile::other", not
    // "<something>/Cookfile::build" — du must not invent a project prefix
    // on top of whatever the engine actually recorded (COOK-333: the
    // engine's own project_id is empty for a local, unconfigured-project
    // build; that's a known, separately-tracked gap, not du's to repair).
    assert!(
        namespaces.iter().any(|(label, _)| label == "/Cookfile::build"),
        "expected the exact namespace '/Cookfile::build' with no invented prefix; got {namespaces:?}\n{text}"
    );
    assert!(
        namespaces.iter().any(|(label, _)| label == "/Cookfile::other"),
        "expected the exact namespace '/Cookfile::other' with no invented prefix; got {namespaces:?}\n{text}"
    );
    // The probe artifact's own namespace ("probe:<key>") is likewise
    // reported verbatim, with no project prefix glued on either.
    assert!(
        namespaces.iter().any(|(label, _)| label == "probe:ns:greet"),
        "expected the exact probe namespace 'probe:ns:greet'; got {namespaces:?}\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Budget line
// ---------------------------------------------------------------------------

#[test]
fn budget_line_reflects_max_size_relative_to_the_seeded_store() {
    let scratch = TempDir::new().unwrap();
    let cache_dir = scratch.path().join("cas");
    let project = seeded_project(&cache_dir);

    assert!(run(project.path(), &["build"]).status.success());
    assert!(run(project.path(), &["other"]).status.success());

    // Far above the store's size (a handful of dozens of bytes): under
    // budget, no OVER BUDGET text, exit 0.
    write_local_cloud_toml(project.path(), &cache_dir, Some("10MB"));
    let under = run(project.path(), &["cache", "du"]);
    assert!(under.status.success(), "du must exit 0 under budget: {under:?}");
    let under_text = String::from_utf8(under.stdout).unwrap();
    assert!(under_text.contains("Budget: "), "missing budget line:\n{under_text}");
    assert!(
        !under_text.contains("OVER BUDGET"),
        "must not claim over-budget when max_size is far above the store size:\n{under_text}"
    );

    // Deliberately tiny: guaranteed smaller than any non-empty seeded
    // store, so this must read as over budget — advisory only, so still
    // exit 0.
    write_local_cloud_toml(project.path(), &cache_dir, Some("1B"));
    let over = run(project.path(), &["cache", "du"]);
    assert!(
        over.status.success(),
        "a budget is advisory; du must not exit non-zero for being over it: {over:?}"
    );
    let over_text = String::from_utf8(over.stdout).unwrap();
    assert!(
        over_text.contains("OVER BUDGET"),
        "expected an OVER BUDGET line with a 1B budget against a nonempty store:\n{over_text}"
    );
}

// ---------------------------------------------------------------------------
// Empty store
// ---------------------------------------------------------------------------

#[test]
fn empty_store_reports_zero_total_without_error() {
    let project = TempDir::new().unwrap();
    // A Cookfile must exist for entry-point discovery to succeed at all
    // (independent of du's own logic) — content is irrelevant since du
    // never registers the workspace.
    fs::write(
        project.path().join("Cookfile"),
        "recipe build\n    test { true }\n",
    )
    .unwrap();
    let cache_dir = project.path().join("does-not-exist");
    write_local_cloud_toml(project.path(), &cache_dir, None);
    assert!(!cache_dir.exists(), "precondition: cache_dir must not exist yet");

    let du = run(project.path(), &["cache", "du"]);
    assert!(du.status.success(), "an absent store is a zero-total report, not an error: {du:?}");
    let text = String::from_utf8(du.stdout).unwrap();
    assert!(
        text.contains("Total: 0 B (0 bytes) across 0 objects"),
        "expected the zero-total line:\n{text}"
    );
    assert!(text.trim().split('\n').next().unwrap().starts_with("Store: "));
}

// ---------------------------------------------------------------------------
// Unparseable budget
// ---------------------------------------------------------------------------

#[test]
fn unparseable_max_size_names_the_offending_literal() {
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("Cookfile"),
        "recipe build\n    test { true }\n",
    )
    .unwrap();
    let cache_dir = project.path().join("cas");
    write_local_cloud_toml(project.path(), &cache_dir, Some("twenty gigs"));

    let du = run(project.path(), &["cache", "du"]);
    assert!(!du.status.success(), "a bad max_size literal must not exit 0: {du:?}");
    let stderr = String::from_utf8_lossy(&du.stderr);
    assert!(
        stderr.contains("twenty gigs"),
        "stderr must name the offending literal; got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Cloud backend path
// ---------------------------------------------------------------------------

#[test]
fn cloud_backend_defers_to_the_server_and_enumerates_nothing_locally() {
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("Cookfile"),
        "recipe build\n    test { true }\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join(".cook")).unwrap();
    // A cache_dir is still declared (isolation is non-negotiable even on a
    // path that's never expected to touch it), but it must stay untouched:
    // the cloud-enabled branch returns before ever calling
    // `resolved_cache_dir`/`LocalBackend::enumerate`.
    let cache_dir = project.path().join("cas");
    let toml = format!(
        "[cloud]\nenabled = true\nendpoint = \"https://example.invalid\"\nproject = \"myproj\"\n\n\
         [cache]\ncache_dir = {:?}\n",
        cache_dir.to_string_lossy()
    );
    fs::write(project.path().join(".cook/cloud.toml"), toml).unwrap();

    // All three of endpoint, project, and an API key are required by
    // `CloudConfig::load_or_default` once `[cloud] enabled = true`.
    let du = run_with_env(
        project.path(),
        &["cache", "du"],
        &[("COOK_CLOUD_API_KEY", "dummy-test-key")],
    );
    assert!(du.status.success(), "cloud-enabled du must exit 0: {du:?}");
    let text = String::from_utf8(du.stdout).unwrap();
    assert!(
        text.contains("cloud cache is server-managed"),
        "expected the server-managed note; got: {text}"
    );
    assert!(
        !cache_dir.exists(),
        "the cloud-enabled path must never create or enumerate the local cache_dir"
    );
}
