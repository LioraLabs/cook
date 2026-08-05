//! `cook why` end to end: edge kinds, aggregation levels, formats, cache
//! tallies, cascade attribution, and timing.
//!
//! Two load-bearing cases.
//!
//! *The engine's edges.* The graph reports the edges the scheduler imposes,
//! read off the engine's own DAG builder (CS-0202). That is additive per
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
         \x20   ingredients \"src.txt\"\n\
         \x20   cook \"mid.txt\" {\n        cat src.txt > mid.txt\n    }\n\
         \n\
         recipe build: gen\n\
         \x20   ingredients \"mid.txt\"\n\
         \x20   cook \"out.txt\" {\n        cat mid.txt > out.txt\n    }\n",
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
    let out = cook(tmp.path(), &["why", "build", "--level", "unit", "--max-nodes", "1"]);
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

    let out = cook(tmp.path(), &["why", "build", "--unit", "build", "--format", "json"]);
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

    let out = cook(tmp.path(), &["why", "build", "--unit", "gen", "--format", "json"]);
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

    let out = cook(tmp.path(), &["why", "build", "--unit", "gen", "--format", "json"]);
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

    let out = cook(tmp.path(), &["why", "build", "--unit", "gen", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let unit = &v["units"][0];
    assert_eq!(unit["local_cause"], "input changed: src.txt", "live: {v}");
    assert_eq!(unit["last_cause"], "input changed: src.txt", "history: {v}");
    // Distinct keys, both present, neither standing in for the other.
    assert!(!unit["local_cause"].is_null() && !unit["last_cause"].is_null(), "{v}");
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
    let out = cook(tmp.path(), &["why", "consumer", "--level", "unit", "--format", "json"]);
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
