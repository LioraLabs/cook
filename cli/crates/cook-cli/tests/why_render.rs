//! `cook why` end to end: edge kinds, aggregation levels, formats, cache
//! tallies, cascade attribution, and timing.
//!
//! Two load-bearing cases.
//!
//! *The engine's edges.* The graph reports the edges the scheduler imposes,
//! read off the shared wiring law (`cook_contracts::unit_graph`, CS-0202) —
//! the same plan the engine lowers into the DAG it executes. That is
//! additive per
//! CS-0161's shipped design: a declared `requires` renders as a barrier
//! whether or not `cook.dep_order` also fine-covers the same producer,
//! because the engine schedules that barrier either way. (An earlier
//! revision of this module doc claimed the opposite — the withdrawn
//! fine-covered narrowing rule — and no test ever pinned it; the graph code
//! that implemented it drifted from the engine, which is COOK-402.) A
//! producer reached only through fine refs still renders `dep_order`-only:
//! nothing coarse was declared, so nothing coarse is imposed or shown.
//!
//! *One command.* The other half is "what will actually run, and why". Before
//! CS-0171 those were `cook dag` and `cook why`, and neither could answer the
//! other's question: the graph had a private local-index-only cache check, and
//! the determinant report had no edges to attribute a miss along.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn cook(root: &Path, args: &[&str]) -> Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(root)
        .output()
        .expect("run cook")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn assert_ok(o: &Output) {
    assert!(
        o.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(o),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Point the shared store at a private per-test directory.
///
/// Without this the host-wide `~/.cache/cook/cloud` serves these deterministic
/// echo/cat units across unrelated test runs, so a "cold" workspace reports
/// hits and the cache assertions below test nothing. Same reasoning as
/// `unit_timing.rs`.
fn isolate_shared_cache(root: &Path) {
    std::fs::create_dir_all(root.join(".cook")).unwrap();
    let shared = root.join(".cook/shared-cache");
    write(
        root,
        ".cook/cloud.toml",
        &format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
    );
}

/// Two recipes joined by a plain dep-list entry and nothing finer.
fn barrier_workspace(root: &Path) {
    isolate_shared_cache(root);
    write(
        root,
        "Cookfile",
        "recipe gen\n    cook \"g.txt\" {\n        echo g > g.txt\n    }\n\n\
         recipe build: gen\n    cook \"a.txt\" {\n        echo a > a.txt\n    }\n",
    );
}

/// A producer and a consumer wired by a real file dependency, so the graph has
/// a data edge to cascade along: `gen` writes `mid.txt` from `src.txt`, and
/// `build` consumes `mid.txt`.
fn chain_workspace(root: &Path) {
    isolate_shared_cache(root);
    write(root, "src.txt", "one\n");
    write(
        root,
        "Cookfile",
        "recipe gen\n\
         \x20   gather \"src.txt\"\n\
         \x20   cook \"mid.txt\" {\n        cat $<in> > mid.txt\n    }\n\
         \n\
         recipe build: gen\n\
         \x20   gather \"mid.txt\"\n\
         \x20   cook \"out.txt\" {\n        cat $<in> > out.txt\n    }\n",
    );
}

#[test]
fn why_renders_the_graph_by_default() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("recipe level"), "{s}");
    assert!(s.starts_with("why build"), "{s}");
}

#[test]
fn recipe_level_is_the_default_and_reports_a_real_barrier() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("recipe level"), "{s}");
    assert!(s.contains("waits on gen"), "{s}");
    // Nothing fine-covers this dep-list edge, so a barrier is the truth.
    assert!(s.contains("barrier"), "{s}");
    assert!(s.contains("free to start immediately"), "{s}");
}

#[test]
fn mermaid_labels_edges_and_weights_barriers() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--format", "mermaid"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.starts_with("graph LR"), "{s}");
    assert!(s.contains("|barrier|"), "{s}");
    assert!(s.contains("==>"), "barrier arrows should be heavy: {s}");
    assert!(s.contains("linkStyle"), "{s}");
}

#[test]
fn json_is_parseable_and_carries_edge_kinds() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    assert_eq!(v["level"], "recipe");
    let edges = v["edges"].as_array().unwrap();
    assert!(edges.iter().any(|e| e["kind"] == "barrier"), "{v}");
}

/// CS-0171: the JSON payload is the successor to *both* former payloads, so
/// the cache tallies must ride alongside the shape.
#[test]
fn json_carries_cache_tallies_alongside_the_shape() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let node = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "recipe:build")
        .expect("build node");
    // Cold workspace: nothing has ever run, so everything rebuilds.
    assert_eq!(node["hits"], 0, "{v}");
    assert_eq!(node["rebuilds"], 1, "{v}");
    // And nothing has ever been observed to take any time.
    assert_eq!(node["observed_ms"], 0, "{v}");
    assert_eq!(node["unobserved"], 1, "{v}");
}

#[test]
fn dot_renders_a_digraph() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--format", "dot"]);
    assert_ok(&out);
    assert!(stdout(&out).starts_with("digraph cook {"));
}

#[test]
fn unknown_level_and_format_are_rejected_by_name() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());

    let out = cook(tmp.path(), &["why", "build", "--level", "nope"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown --level 'nope'"));

    let out = cook(tmp.path(), &["why", "build", "--format", "nope"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown --format 'nope'"));
}

#[test]
fn unit_level_refuses_past_max_nodes_rather_than_emitting_a_blob() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(
        tmp.path(),
        &["why", "build", "--level", "unit", "--max-nodes", "1"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not readable in any format"), "{err}");
    // The refusal must point at the levels that do work on the same graph.
    assert!(err.contains("--level recipe"), "{err}");
}

// ---------------------------------------------------------------------------
// CS-0171: the merge
// ---------------------------------------------------------------------------

/// `cook dag` is gone. Not aliased, not deprecated — removed, because it never
/// shipped in a tagged release.
#[test]
fn cook_dag_no_longer_exists() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["dag", "build"]);
    assert!(!out.status.success(), "`cook dag` must not resolve");
    // It falls through to recipe dispatch and fails as an unknown recipe,
    // rather than being caught as a reserved subcommand.
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("dag"), "{err}");
}

/// The determinant fidelity CS-0112 specified must survive the merge. At unit
/// level — where the node count is already capped — the full per-unit block
/// prints under the graph.
#[test]
fn unit_level_still_reports_full_determinants() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--level", "unit"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("unit level"), "{s}");
    assert!(s.contains("command_hash"), "determinants missing: {s}");
    assert!(s.contains("env_contribution"), "{s}");
    assert!(s.contains("seal_contribution"), "{s}");
    assert!(s.contains("inputs:"), "{s}");
    assert!(s.contains("src.txt"), "{s}");
}

/// The `--unit` selector answers a determinant question directly, without
/// making the caller render the whole closure at unit granularity.
#[test]
fn unit_selector_reports_determinants_for_one_unit() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--unit", "out.txt"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("command_hash"), "{s}");
    assert!(s.contains("out.txt"), "{s}");
    // Exactly one unit is selected: the report has one determinant block.
    assert_eq!(s.matches("command_hash").count(), 1, "selector should narrow: {s}");
}

/// §17.1.6.6 / CS-0217: `cook why` emits its machine-readable answer as two
/// documents — the whole closure, and the subset a `--unit` selector names —
/// and a consumer must be able to migrate BOTH. The selector-scoped document
/// carried no version at all until CS-0217, so a tool reading it had no way to
/// tell a payload change from a payload it had misparsed.
///
/// The two versions are asserted equal, not merely present: they are the same
/// wire format at two scopes, and two numbers would be free to drift the
/// moment one of them mattered.
#[test]
fn both_machine_readable_documents_carry_the_same_schema_version() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());

    let whole = cook(
        tmp.path(),
        &["why", "build", "--level", "unit", "--format", "json"],
    );
    assert_ok(&whole);
    let whole: serde_json::Value = serde_json::from_str(&stdout(&whole)).expect("valid json");
    let version = whole["schema_version"].clone();
    assert!(version.is_number(), "the closure document must carry a version: {whole}");

    let selected = cook(
        tmp.path(),
        &["why", "build", "--unit", "out.txt", "--format", "json"],
    );
    assert_ok(&selected);
    let selected: serde_json::Value =
        serde_json::from_str(&stdout(&selected)).expect("valid json");
    assert_eq!(
        selected["schema_version"], version,
        "the selector document must carry the same wire-format version as the closure \
         document it is a subset of: {selected}"
    );
    // And it is still the document it was: a version is added, nothing moves.
    assert_eq!(selected["recipe"], "build", "{selected}");
    assert_eq!(selected["units"].as_array().map(Vec::len), Some(1), "{selected}");
}

/// A selector matching nothing is a user error worth naming, not an empty
/// report that reads as "nothing to explain".
#[test]
fn a_unit_selector_matching_nothing_is_an_error() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build", "--unit", "nosuchthing"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("matched no unit"), "{err}");
}

/// §17.1.6.2: a coarse node reports counts, and on a warm workspace they are
/// hits rather than rebuilds. This is the merge working: the tally comes from
/// `why`'s two-tier verdict, not from the deleted local-only check.
///
/// Builds TWICE to reach steady state, which is a workaround for COOK-326: a
/// recipe consuming another recipe's generated output does not record a
/// correct cache entry on its first run (and is served wrong bytes from the
/// shared store). One build is enough once that is fixed, and this test should
/// be tightened back to one then.
#[test]
fn a_warm_workspace_reports_hits_not_rebuilds() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());

    let cold = cook(tmp.path(), &["why", "build"]);
    assert_ok(&cold);
    assert!(stdout(&cold).contains("2 rebuild"), "cold: {}", stdout(&cold));

    assert_ok(&cook(tmp.path(), &["build"]));
    assert_ok(&cook(tmp.path(), &["build"]));

    let warm = cook(tmp.path(), &["why", "build"]);
    assert_ok(&warm);
    let s = stdout(&warm);
    assert!(s.contains("2 hit, 0 rebuild"), "warm: {s}");
    assert!(!s.contains("[1 rebuild]"), "no node should rebuild: {s}");
}

/// §17.1.6.3 and §17.1.6.4 together: after editing the root source, the graph
/// reports what rebuilds, what that rebuild forces, and what it was observed
/// to cost last time.
#[test]
fn an_edited_input_reports_cascade_and_observed_timing() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));

    write(tmp.path(), "src.txt", "two\n");

    let out = cook(tmp.path(), &["why", "build", "--level", "unit"]);
    assert_ok(&out);
    let s = stdout(&out);

    // Both units rebuild: the edit invalidates mid.txt, which invalidates out.txt.
    assert!(s.contains("2 rebuild"), "{s}");
    // The upstream names what its rebuild costs downstream.
    assert!(
        s.contains("invalidates 1 downstream unit"),
        "cascade attribution missing: {s}"
    );
    // And the downstream names the upstream rather than presenting its miss as
    // an independent finding.
    assert!(s.contains("← rebuilding"), "upstream not marked: {s}");
    // The prior run timed both units, so both carry an observation.
    assert!(s.contains("observed"), "timing missing: {s}");
    assert!(
        !s.contains("estimate") && !s.contains("will take"),
        "timing must not read as a prediction: {s}"
    );
}

/// §17.1.6.4: a workspace that has never run has no timings, and must say so
/// by omission rather than by rendering zero.
#[test]
fn a_never_run_workspace_reports_no_timing_rather_than_zero() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    let out = cook(tmp.path(), &["why", "build"]);
    assert_ok(&out);
    let s = stdout(&out);
    // No unit has ever run, so no node carries a duration at all. Checked as
    // a whole-output property rather than `!contains("0ms observed")`, which
    // any duration ending in zero would satisfy ("400ms observed").
    assert!(!s.contains("observed"), "absence is not zero: {s}");
}

/// Drop the local index and the built outputs, keeping the shared store and its
/// config. This is a fresh checkout on a machine whose cache is warm: the exact
/// shape someone evaluating Cook is in when they first ask what a build will do.
fn go_cold_keeping_shared_cache(root: &Path) {
    std::fs::remove_dir_all(root.join(".cook/cache")).unwrap();
    for f in ["mid.txt", "out.txt"] {
        let _ = std::fs::remove_file(root.join(f));
    }
}

/// CS-0173: the headline tally must match what the build then does.
///
/// Before CS-0173 `build` reported a miss here, because `mid.txt` had not been
/// restored yet and classification hashed the working tree. Its producer is a
/// cache hit, so those bytes were already determined; only evaluation order
/// hid them.
#[test]
fn cold_tree_with_a_warm_shared_cache_predicts_the_hits_it_will_get() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    go_cold_keeping_shared_cache(tmp.path());

    let out = cook(tmp.path(), &["why", "build", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    for id in ["recipe:gen", "recipe:build"] {
        let node = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap_or_else(|| panic!("{id} node in {v}"));
        assert_eq!(node["hits"], 1, "{id} should be predicted a hit: {v}");
        assert_eq!(node["rebuilds"], 0, "{id} should not rebuild: {v}");
    }
}

/// The other half of the same defect. A unit whose input is about to be
/// rewritten was reported as a hit, because the stale bytes on disk still
/// matched its recorded key.
#[test]
fn a_unit_downstream_of_a_rebuild_is_not_reported_as_a_hit() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    // Change the root source. `gen` must rerun, so `mid.txt` (still on disk,
    // still matching `build`'s recorded key) is about to change underneath it.
    write(tmp.path(), "src.txt", "two\n");

    let out = cook(tmp.path(), &["why", "build", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let build = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "recipe:build")
        .expect("build node");
    assert_eq!(build["hits"], 0, "downstream of a rebuild is not a hit: {v}");
    assert_eq!(build["rebuilds"], 1, "{v}");
}

/// A forced unit has no key. The wire format must say so with null rather than
/// an empty or fabricated string, and must name the cause.
#[test]
fn a_forced_unit_reports_no_key_and_names_its_cause() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    write(tmp.path(), "src.txt", "two\n");

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "build", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["status"] == "forced_by_upstream")
        .unwrap_or_else(|| panic!("a forced unit in {v}"));
    assert!(unit["key"].is_null(), "no key for a forced unit: {v}");
    assert_eq!(unit["forced_by"], "gen", "{v}");
    assert_eq!(unit["pending_input_path"], "mid.txt", "{v}");
    // The pending input is reported as pending, NOT as an input with a hash.
    assert!(
        unit["determinants"]["inputs"].get("mid.txt").is_none(),
        "a pending input must not carry a hash: {v}"
    );
    assert_eq!(unit["determinants"]["pending_inputs"]["mid.txt"], "gen", "{v}");
}

/// The plain renderer names the upstream instead of restating the consequence.
#[test]
fn plain_output_attributes_a_forced_rebuild_to_its_upstream() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    write(tmp.path(), "src.txt", "two\n");

    let out = cook(tmp.path(), &["why", "build", "--unit", "build"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("REBUILD (forced by gen)"), "{s}");
    assert!(s.contains("key not computable"), "{s}");
    assert!(s.contains("mid.txt  pending gen"), "{s}");
}

/// CS-0174: a local miss must name the determinant that moved. The reason was
/// already computed inside `local_step_hit` and discarded, which left the
/// explain tool strictly less informative than the build log it pre-empts:
/// a shared miss got a manifest diff, a local miss got a list of determinants
/// and no verdict.
#[test]
fn a_local_miss_names_the_determinant_that_changed() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    write(tmp.path(), "src.txt", "two\n");

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "gen", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_hit"], false, "{v}");
    assert_eq!(unit["local_cause"], "input changed: src.txt", "{v}");
}

/// The same attribution, in the plain renderer.
#[test]
fn plain_output_names_the_local_miss_cause() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    write(tmp.path(), "src.txt", "two\n");

    let out = cook(tmp.path(), &["why", "build", "--unit", "gen"]);
    assert_ok(&out);
    assert!(
        stdout(&out).contains("local-miss cause: input changed: src.txt"),
        "{}",
        stdout(&out)
    );
}

/// CS-0174: for a unit that is currently a hit there is no live cause to
/// report, and the retained log is the only thing that can say why it last
/// ran. That is the "why did this rebuild overnight when I changed nothing"
/// question, and it must be labelled as history rather than as a verdict.
#[test]
fn a_hit_reports_why_it_last_ran_from_the_recorded_observation() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    write(tmp.path(), "src.txt", "two\n");
    // This build records the cause; afterwards the unit is a hit again.
    assert_ok(&cook(tmp.path(), &["build"]));

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "gen", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_hit"], true, "should be a hit now: {v}");
    // No live cause: nothing is going to rebuild.
    assert!(unit["local_cause"].is_null(), "{v}");
    // But history knows why it ran.
    assert_eq!(unit["last_cause"], "input changed: src.txt", "{v}");
    assert!(
        unit["last_cause_recorded_at"].as_u64().unwrap_or(0) > 0,
        "{v}"
    );
    assert_eq!(unit["recorded_log_bytes"], 0, "{v}");
}

/// The two causes answer different questions and must never be conflated: one
/// is a verdict on the run being explained, the other is a record of a past
/// one. A unit that will rebuild for a *new* reason must not have its live
/// cause overwritten by the stale one.
#[test]
fn live_and_historical_causes_are_reported_independently() {
    let tmp = TempDir::new().unwrap();
    chain_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));
    // First edit, then build: the log now records "src.txt".
    write(tmp.path(), "src.txt", "two\n");
    assert_ok(&cook(tmp.path(), &["build"]));
    // Second edit, NOT built: the live cause is about to be recomputed while
    // history still remembers the previous run.
    write(tmp.path(), "src.txt", "three\n");

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "gen", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_cause"], "input changed: src.txt", "live: {v}");
    assert_eq!(unit["last_cause"], "input changed: src.txt", "history: {v}");
    // Distinct keys, both present, neither standing in for the other.
    assert!(!unit["local_cause"].is_null() && !unit["last_cause"].is_null(), "{v}");
}

/// CS-0245 / COOK-529's own reason to exist, and the flip check for it: on
/// `189fc4a9` `cook why` reads a sealed `files` probe's value from
/// `.cook/probes/<key>.json`, which the LAST `cook build` wrote — so editing a
/// sealed file after a build and asking `cook why` reports a stale `HIT`, while
/// the next `cook build` prints `rebuild (seal changed)`. §17.1.6.1 now
/// requires a `files`/`tools` declaration to be resolved FRESH at query time,
/// so the two must agree.
#[test]
fn a_moved_sealed_files_probe_reports_a_local_seal_changed_miss() {
    let tmp = TempDir::new().unwrap();
    isolate_shared_cache(tmp.path());
    write(tmp.path(), "src/a.ts", "one\n");
    write(
        tmp.path(),
        "Cookfile",
        "files srcs\n    \"src/*.ts\"\n\n\
         recipe build\n    seal srcs\n    cook \"out.txt\" {\n        echo built > out.txt\n    }\n",
    );
    assert_ok(&cook(tmp.path(), &["build"]));

    // Edit a file the `srcs` declaration matches. `.cook/probes/srcs.json`
    // still holds the OLD hash — nothing re-runs `srcs` until something
    // consults it again.
    write(tmp.path(), "src/a.ts", "two\n");

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "out.txt", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_hit"], false, "{v}");
    assert_eq!(unit["local_cause"], "seal changed", "{v}");
}

// ---------------------------------------------------------------------------
// COOK-529 / CS-0245: the delta behind the miss, not the whole sealed value
// ---------------------------------------------------------------------------

/// Three files sealed under one `files` declaration, so a delta test can tell
/// the difference between "named the one that moved" and "dumped the set".
fn three_sealed_files_workspace(root: &Path) {
    isolate_shared_cache(root);
    write(root, "src/a.ts", "a-one\n");
    write(root, "src/b.ts", "b-one\n");
    write(root, "src/c.ts", "c-one\n");
    write(
        root,
        "Cookfile",
        "files srcs\n    \"src/*.ts\"\n\n\
         recipe build\n    seal srcs\n    cook \"out.txt\" {\n        echo built > out.txt\n    }\n",
    );
}

/// The block of lines directly under `  local-miss cause: seal changed`,
/// i.e. everything indented one level deeper than the top-level determinant
/// fields (`>= 4` leading spaces) until the first line that returns to that
/// shallower indentation. Isolating this block is what makes the negative
/// assertion in the next test meaningful: the untouched `sealed probes:`
/// full-value dump (§17.1.6.1, kept deliberately per this ticket's Do Not
/// list) legitimately mentions every file, so a whole-stdout `!contains`
/// check would prove nothing.
fn seal_delta_block(stdout: &str) -> String {
    let mut in_block = false;
    let mut out = String::new();
    for line in stdout.lines() {
        if line == "  local-miss cause: seal changed" {
            in_block = true;
            continue;
        }
        if in_block {
            if line.starts_with("    ") {
                out.push_str(line);
                out.push('\n');
            } else {
                break;
            }
        }
    }
    out
}

/// Test 1 (spec Evidence #1): edit ONE of three sealed files. The delta block
/// names the probe key and the changed file, and does NOT name the two
/// unchanged files — the negative half that makes this a delta rather than a
/// dump.
///
/// M-5 (seam review on 9eb2faf7): this negative half was mutation-tested BY
/// HAND, not by a second `#[test]` in this file — an earlier revision of this
/// comment cited a test named `the_delta_block_negative_half_is_load_bearing`
/// that does not exist here. The mutation: reverting `render_probe_delta_lines`
/// (`why_render.rs`) to print the whole `sealed_probes` value again instead of
/// the per-entry delta makes the `!block.contains("src/b.ts")` /
/// `!block.contains("src/c.ts")` assertions below fail, which is what proves
/// they are load-bearing rather than vacuously true.
#[test]
fn plain_text_local_miss_names_the_changed_file_and_not_the_others() {
    let tmp = TempDir::new().unwrap();
    three_sealed_files_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));

    write(tmp.path(), "src/a.ts", "a-two\n");

    let out = cook(tmp.path(), &["why", "build", "--unit", "out.txt"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("local-miss cause: seal changed"), "{s}");

    let block = seal_delta_block(&s);
    assert!(!block.is_empty(), "expected a delta block under the cause line: {s}");
    assert!(block.contains("srcs:"), "probe key must be named: {block}");
    assert!(block.contains("src/a.ts"), "changed path must be named: {block}");
    assert!(!block.contains("src/b.ts"), "unchanged path leaked into the delta: {block}");
    assert!(!block.contains("src/c.ts"), "unchanged path leaked into the delta: {block}");
}

/// Test 2 (spec Evidence #2): a file added to the sealed set and a file
/// removed from it are each named as such, distinctly from a change.
#[test]
fn plain_text_local_miss_names_an_added_and_a_removed_file() {
    let tmp = TempDir::new().unwrap();
    three_sealed_files_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));

    std::fs::remove_file(tmp.path().join("src/b.ts")).unwrap();
    write(tmp.path(), "src/d.ts", "d-one\n");

    let out = cook(tmp.path(), &["why", "build", "--unit", "out.txt"]);
    assert_ok(&out);
    let s = stdout(&out);
    let block = seal_delta_block(&s);
    assert!(block.contains("- src/b.ts") || block.contains("-b.ts"), "removed not named: {block}");
    assert!(block.contains("+ src/d.ts") || block.contains("+d.ts"), "added not named: {block}");
    assert!(block.contains("src/b.ts"), "removed path must be named: {block}");
    assert!(block.contains("src/d.ts"), "added path must be named: {block}");
    // a.ts and c.ts never moved: not in the delta block.
    assert!(!block.contains("src/a.ts"), "{block}");
    assert!(!block.contains("src/c.ts"), "{block}");
}

/// Test 3 (spec Evidence #3): the JSON `seal_deltas` structure carries the
/// probe key and the changed entry as data, not as the rendered string.
#[test]
fn json_local_miss_carries_the_seal_delta_as_structured_data() {
    let tmp = TempDir::new().unwrap();
    three_sealed_files_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));

    write(tmp.path(), "src/a.ts", "a-two\n");

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "out.txt", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_cause"], "seal changed", "{v}");
    let deltas = unit["seal_deltas"].as_array().expect("seal_deltas array");
    assert_eq!(deltas.len(), 1, "{v}");
    let d = &deltas[0];
    assert_eq!(d["key"], "srcs", "{v}");
    assert_eq!(d["kind"], "entries", "{v}");
    let changed = d["changed"].as_array().expect("changed array");
    assert_eq!(changed.len(), 1, "{v}");
    assert_eq!(changed[0]["key"], "src/a.ts", "{v}");
    assert!(changed[0]["old"].is_string() && changed[0]["new"].is_string(), "{v}");
    assert_ne!(changed[0]["old"], changed[0]["new"], "{v}");
    // The two untouched files are absent from the delta entirely.
    assert!(d["added"].as_array().unwrap().is_empty(), "{v}");
    assert!(d["removed"].as_array().unwrap().is_empty(), "{v}");
}

/// Test 4 (spec Evidence #4): with `.cook/probes/` deleted between the build
/// and the query, the report says first observation and fabricates no old
/// value. Deleting the record alone is not enough to produce a local miss
/// (CS-0245's fresh-resolution recomputes the same bytes from an unchanged
/// tree), so the file is also edited — the genuine local-miss case whose
/// "prior" side this ticket's snapshot can no longer see.
#[test]
fn first_observation_is_reported_when_the_prior_record_is_gone() {
    let tmp = TempDir::new().unwrap();
    three_sealed_files_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));

    std::fs::remove_dir_all(tmp.path().join(".cook/probes")).unwrap();
    write(tmp.path(), "src/a.ts", "a-two\n");

    let out = cook(
        tmp.path(),
        &["why", "build", "--unit", "out.txt", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_cause"], "seal changed", "{v}");
    let deltas = unit["seal_deltas"].as_array().expect("seal_deltas array");
    assert_eq!(deltas.len(), 1, "{v}");
    assert_eq!(deltas[0]["kind"], "first_observation", "{v}");
    // No fabricated old value: a first-observation entry has no old/new/
    // added/removed/changed payload at all.
    assert!(deltas[0].get("old").is_none(), "{v}");
    assert!(deltas[0].get("changed").is_none(), "{v}");

    let out = cook(tmp.path(), &["why", "build", "--unit", "out.txt"]);
    assert_ok(&out);
    let block = seal_delta_block(&stdout(&out));
    assert!(block.contains("first observation"), "{block}");
    assert!(!block.contains("->"), "must not render a diff: {block}");
}

/// §17.1.6.1's I5: two recipes seal the SAME probe independently. Build `one`
/// (its index now expects the probe's value at that moment). Edit the sealed
/// file, then build `two` (a DIFFERENT recipe) — this reaches the probe and
/// rewrites `.cook/probes/srcs.json` to the new value, but never touches
/// `one`'s own index record. Querying `one` now finds a genuine seal-changed
/// local miss (the value the index recorded no longer matches), yet the
/// snapshot taken before this query's registration and the value resolved
/// fresh are IDENTICAL (both are what `two`'s build already wrote) — the
/// delta between them is empty even though the miss is real. §17.1.6.1
/// requires the seal set be named as the differing determinant without a
/// probe key or entry, and forbids presenting this as an empty diff.
#[test]
fn a_genuine_seal_changed_miss_with_no_nameable_entry_reads_as_such() {
    let tmp = TempDir::new().unwrap();
    isolate_shared_cache(tmp.path());
    write(tmp.path(), "src/a.ts", "one\n");
    write(
        tmp.path(),
        "Cookfile",
        "files srcs\n    \"src/*.ts\"\n\n\
         recipe one\n    seal srcs\n    cook \"out1.txt\" {\n        echo built1 > out1.txt\n    }\n\n\
         recipe two\n    seal srcs\n    cook \"out2.txt\" {\n        echo built2 > out2.txt\n    }\n",
    );
    assert_ok(&cook(tmp.path(), &["one"]));
    write(tmp.path(), "src/a.ts", "two\n");
    assert_ok(&cook(tmp.path(), &["two"]));

    let out = cook(
        tmp.path(),
        &["why", "one", "--unit", "out1.txt", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_cause"], "seal changed", "genuine miss: {v}");
    assert_eq!(
        unit["seal_deltas"].as_array().unwrap().len(),
        0,
        "nothing nameable — this is I5, not a bug: {v}"
    );

    let out = cook(tmp.path(), &["why", "one", "--unit", "out1.txt"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("local-miss cause: seal changed"), "{s}");
    // The seal set is named as the differing determinant without a probe key
    // or entry — not silence, and not an empty/partial diff presented as the
    // full account.
    let block = seal_delta_block(&s);
    assert!(
        block.contains("no probe or entry can be named"),
        "must say WHY nothing is nameable, not print nothing: {block}"
    );
}

// ---------------------------------------------------------------------------
// CS-0202: the graph reports the engine's edges
// ---------------------------------------------------------------------------

/// A declared `cook.require_recipe` fine-covered by `cook.dep_order` on the
/// same producer. The engine keeps the whole-recipe barrier (CS-0161 is
/// strictly additive), so the graph must render BOTH kinds. Before CS-0202
/// the barrier was suppressed whenever any unit fine-covered the producer.
fn additive_workspace(root: &Path) {
    isolate_shared_cache(root);
    write(
        root,
        "Cookfile",
        "recipe producer\n\
         \x20   cook \"g.txt\" {\n        echo g > g.txt\n    }\n\
         \n\
         recipe consumer\n\
         \x20   cook.require_recipe(\"producer\")\n\
         \x20   cook \"first.txt\" {\n        echo a > first.txt\n    }\n\
         \x20   cook.dep_order(\"producer\")\n\
         \x20   cook \"out.txt\" {\n        echo b > out.txt\n    }\n",
    );
}

#[test]
fn a_declared_barrier_renders_alongside_its_fine_cover() {
    let tmp = TempDir::new().unwrap();
    additive_workspace(tmp.path());
    let out = cook(
        tmp.path(),
        &["why", "consumer", "--level", "unit", "--format", "json"],
    );
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    let edges = v["edges"].as_array().unwrap();
    assert!(
        edges.iter().any(|e| e["kind"] == "barrier"),
        "the declared require_recipe barrier must render (additive, CS-0161): {v}"
    );
    assert!(
        edges.iter().any(|e| e["kind"] == "dep_order"),
        "the fine ref must render too: {v}"
    );
}

/// A dependency routed through a unit-less meta-target (`recipe middle :
/// producer` with no body). The engine forwards the producer's leaves through
/// the empty barrier; before CS-0202 the graph recorded no terminals for the
/// middle recipe and the dependency vanished from `cook why` entirely.
#[test]
fn a_dep_through_a_unit_less_meta_target_is_not_hidden() {
    let tmp = TempDir::new().unwrap();
    isolate_shared_cache(tmp.path());
    write(
        tmp.path(),
        "Cookfile",
        "recipe producer\n\
         \x20   cook \"g.txt\" {\n        echo g > g.txt\n    }\n\
         \n\
         recipe middle: producer\n\
         \n\
         recipe consumer: middle\n\
         \x20   cook \"a.txt\" {\n        echo a > a.txt\n    }\n",
    );
    let out = cook(tmp.path(), &["why", "consumer", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    let edges = v["edges"].as_array().unwrap();
    assert!(
        edges.iter().any(|e| e["from"] == "recipe:producer"
            && e["to"] == "recipe:consumer"
            && e["kind"] == "barrier"),
        "the dep must forward through the unit-less middle to the real \
         producer: {v}"
    );
}

// ---------------------------------------------------------------------------
// C-1 (seam review on 9eb2faf7): a workspace with an IMPORT.
//
// Every other test in this file drives a single root Cookfile — the shape
// under which `import_prefix` collapses to `""` regardless of whether it is
// derived from `ProbeUnit.key` (Cookfile-LOCAL, never restamped by
// `cook-plan`'s registration merge) or from the map key
// `registered_workspace.probes` is actually keyed by (workspace-QUALIFIED,
// `cook-plan/src/registers.rs`'s `qualified_key(prefix, &key)`). Deriving
// the prefix from `pu.key` returns `""` for EVERY probe in EVERY workspace,
// so a bug in that derivation is invisible to a single-Cookfile test: `""`
// happens to be correct there. Only an import makes the two derivations
// disagree.
// ---------------------------------------------------------------------------

/// Root imports `./api`; the member declares a top-level `files` probe over
/// its OWN tree (`api/src/*.ts`) and a recipe that seals it. Mirrors
/// `cook-cli/tests/why.rs`'s `ROOT_COOKFILE`/`API_COOKFILE` import shape,
/// with the member's recipe additionally sealing a `files` probe.
fn import_with_member_files_probe_workspace(root: &Path) {
    isolate_shared_cache(root);
    write(
        root,
        "Cookfile",
        "import api ./api\n\n\
         recipe build: api.compile\n\
         \x20   cook \"build/top.txt\" {\n        mkdir -p build\n        echo top > $<out>\n    }\n",
    );
    write(
        root,
        "api/Cookfile",
        "files srcs\n    \"src/*.ts\"\n\n\
         recipe compile\n\
         \x20   seal srcs\n\
         \x20   cook \"build/api-build.stamp\" {\n        mkdir -p build\n        echo stamp > $<out>\n    }\n",
    );
    write(root, "api/src/a.ts", "a-one\n");
}

/// The C-1 repro, through the real binary. On `9eb2faf7` this fails BOTH
/// ways: `local_hit` is `false` with a fabricated `local-miss cause: seal
/// changed` and a `seal_deltas` entry naming a hash movement that never
/// happened (evidence #1), and the member probe's own sealed value folds
/// every path in `api/src/*.ts` to `"<missing>"` because the glob was
/// resolved against the ROOT Cookfile's working directory instead of the
/// member's (evidence #2) — `import_prefix` collapsed to `""` for a
/// Cookfile-local probe key that never contains a `.`.
#[test]
fn import_member_probe_is_not_fabricated_as_a_miss() {
    let tmp = TempDir::new().unwrap();
    import_with_member_files_probe_workspace(tmp.path());
    assert_ok(&cook(tmp.path(), &["build"]));

    // Nothing edited since the build that just ran.
    let out = cook(tmp.path(), &["why", "build", "--level", "unit", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    let unit = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["recipe"] == "api.compile")
        .unwrap_or_else(|| panic!("expected an api.compile unit: {v}"));

    // 1. No fabricated miss: the member's sealing unit is a clean local hit,
    // with no local-miss cause and no seal delta.
    assert_eq!(
        unit["local_hit"], true,
        "member unit must be a local hit with nothing edited: {v}"
    );
    assert!(unit["local_cause"].is_null(), "no local-miss cause on a hit: {v}");
    assert_eq!(
        unit["seal_deltas"].as_array().unwrap().len(),
        0,
        "no seal delta on a hit: {v}"
    );

    // 2. The member probe's sealed value carries ITS OWN real content hash —
    // not the `"<missing>"` fold a glob resolved against the wrong working
    // directory produces.
    let srcs_raw = unit["determinants"]["sealed_probes"]["srcs"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a sealed_probes.srcs string: {v}"));
    assert!(
        !srcs_raw.contains("<missing>"),
        "member probe must resolve its OWN files, not the root's: {srcs_raw}"
    );
    let srcs_value: serde_json::Value =
        serde_json::from_str(srcs_raw).expect("sealed_probes.srcs is canonical JSON");
    let hash = srcs_value["src/a.ts"]
        .as_str()
        .unwrap_or_else(|| panic!("expected src/a.ts in the files manifest: {srcs_raw}"));
    assert_eq!(hash.len(), 64, "expected a sha256 hex digest, got: {hash}");
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "expected a hex digest, got: {hash}"
    );
}
