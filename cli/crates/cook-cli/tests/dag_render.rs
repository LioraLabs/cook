//! `cook dag` rendering: edge kinds, aggregation levels, and formats.
//!
//! The load-bearing case is fine-coverage. A recipe-level dependency that a
//! module has covered with per-unit `cook.dep_order` references (CS-0161) does
//! not execute as a whole-recipe barrier, so reporting one would be a lie —
//! and an expensive one, since "why is my build serial" is the question this
//! command exists to answer.

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

/// Two recipes joined by a plain dep-list entry and nothing finer.
fn barrier_workspace(root: &Path) {
    write(
        root,
        "Cookfile",
        "recipe gen\n    cook \"g.txt\" {\n        echo g > g.txt\n    }\n\n\
         recipe build: gen\n    cook \"a.txt\" {\n        echo a > a.txt\n    }\n",
    );
}

#[test]
fn dag_runs_without_the_viewer_feature() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["dag", "build"]);
    assert_ok(&out);
    assert!(
        !stdout(&out).contains("--features viewer"),
        "the default binary must render a graph, not ask to be rebuilt"
    );
}

#[test]
fn recipe_level_is_the_default_and_reports_a_real_barrier() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["dag", "build"]);
    assert_ok(&out);
    let s = stdout(&out);
    assert!(s.contains("recipe level"), "{s}");
    assert!(s.contains("waits on gen"), "{s}");
    // Nothing fine-covers this dep-list edge, so a barrier is the truth.
    assert!(s.contains("barrier"), "{s}");
    assert!(s.contains("(waits on nothing)"), "{s}");
}

#[test]
fn mermaid_labels_edges_and_weights_barriers() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["dag", "build", "--format", "mermaid"]);
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
    let out = cook(tmp.path(), &["dag", "build", "--format", "json"]);
    assert_ok(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    assert_eq!(v["level"], "recipe");
    let edges = v["edges"].as_array().unwrap();
    assert!(edges.iter().any(|e| e["kind"] == "barrier"), "{v}");
}

#[test]
fn dot_renders_a_digraph() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["dag", "build", "--format", "dot"]);
    assert_ok(&out);
    assert!(stdout(&out).starts_with("digraph cook {"));
}

#[test]
fn unknown_level_and_format_are_rejected_by_name() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());

    let out = cook(tmp.path(), &["dag", "build", "--level", "nope"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown --level 'nope'"));

    let out = cook(tmp.path(), &["dag", "build", "--format", "nope"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown --format 'nope'"));
}

#[test]
fn unit_level_refuses_past_max_nodes_rather_than_emitting_a_blob() {
    let tmp = TempDir::new().unwrap();
    barrier_workspace(tmp.path());
    let out = cook(tmp.path(), &["dag", "build", "--level", "unit", "--max-nodes", "1"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not readable in any format"), "{err}");
    // The refusal must point at the levels that do work on the same graph.
    assert!(err.contains("--level recipe"), "{err}");
}
