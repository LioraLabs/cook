//! Surface e2e conformance harness.
//!
//! Runs the real `cook` binary (fresh by construction: CARGO_BIN_EXE_cook
//! forces cargo to rebuild the bin before this test compiles) against every
//! fixture directory under cli/e2e-fixtures/surface/. Each fixture is copied
//! into a tempdir with an isolated artifact/probe cache (via .cook/cloud.toml
//! [cache] cache_dir), so the user's shared ~/.cache/cook/cloud is never
//! touched. A fixture = Cookfile + inputs + expect.toml describing one or
//! more sequential `cook` runs with exit-code and filesystem assertions.
//!
//! expect.toml schema:
//!   xfail = "<ISSUE-KEY>"  (optional) fixture pins a known-open bug; it must
//!                          FAIL. When the fix lands the harness reports XPASS
//!                          as a failure and you delete this line.
//!   [[run]]                one entry per sequential invocation
//!   args = ["build"]       argv after `cook`
//!   cwd = "sub/dir"        (optional) run cook from this subdir of the fixture
//!   exit = 0               (optional) expected exit code, default 0
//!   env = { K = "v" }      (optional) extra process env
//!   write = { path = "content" }   (optional) files (re)written BEFORE the run
//!   [run.assert]           all fields optional
//!   exists / absent = ["path", ...]
//!   equals / contains = { path = "text" }
//!   unchanged / changed = ["path", ...]   byte-compare vs the previous run
//!   output_contains = ["text", ...]       matched against stdout+stderr

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    xfail: Option<String>,
    #[serde(rename = "run")]
    runs: Vec<RunSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunSpec {
    args: Vec<String>,
    cwd: Option<String>,
    #[serde(default)]
    exit: i32,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    write: BTreeMap<String, String>,
    #[serde(default)]
    assert: AssertSpec,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AssertSpec {
    #[serde(default)]
    exists: Vec<String>,
    #[serde(default)]
    absent: Vec<String>,
    #[serde(default)]
    equals: BTreeMap<String, String>,
    #[serde(default)]
    contains: BTreeMap<String, String>,
    #[serde(default)]
    unchanged: Vec<String>,
    #[serde(default)]
    changed: Vec<String>,
    #[serde(default)]
    output_contains: Vec<String>,
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../e2e-fixtures/surface")
        .canonicalize()
        .expect("surface corpus root missing: cli/e2e-fixtures/surface")
}

fn copy_tree(src: &Path, dst: &Path) {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry.expect("walk fixture");
        let rel = entry.path().strip_prefix(src).unwrap();
        if rel.as_os_str().is_empty() || rel == Path::new("expect.toml") {
            continue;
        }
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).expect("mkdir");
        } else {
            fs::create_dir_all(target.parent().unwrap()).expect("mkdir parent");
            fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

/// Paths whose bytes are snapshotted after every run: anything any run
/// asserts `changed` or `unchanged` against.
fn tracked_paths(manifest: &Manifest) -> BTreeSet<String> {
    manifest
        .runs
        .iter()
        .flat_map(|r| r.assert.changed.iter().chain(r.assert.unchanged.iter()))
        .cloned()
        .collect()
}

fn run_fixture(manifest: &Manifest, fixture: &Path) -> Result<(), String> {
    if manifest.runs.is_empty() {
        return Err("expect.toml declares no [[run]] entries".into());
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj = tmp.path().join("proj");
    copy_tree(fixture, &proj);
    // Isolate the persistent artifact/probe cache: without this the run
    // would hit the user's shared ~/.cache/cook/cloud.
    fs::create_dir_all(proj.join(".cook")).expect("mkdir .cook");
    fs::write(
        proj.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = \"{}\"\n", tmp.path().join("cache").display()),
    )
    .expect("write cloud.toml");

    let tracked = tracked_paths(manifest);
    let mut snapshot: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();

    for (i, run) in manifest.runs.iter().enumerate() {
        let label = format!("run[{i}] `cook {}`", run.args.join(" "));
        for (path, content) in &run.write {
            let p = proj.join(path);
            fs::create_dir_all(p.parent().unwrap()).expect("mkdir for write");
            fs::write(&p, content).expect("pre-run write");
        }
        let out = match Command::new(env!("CARGO_BIN_EXE_cook"))
            .args(&run.args)
            .current_dir(proj.join(run.cwd.as_deref().unwrap_or(".")))
            .env("NO_COLOR", "1")
            .env("NO_PROGRESS", "1")
            .env("CI", "1")
            // Defense-in-depth: if a run's workspace-root resolution ever
            // misses the harness cloud.toml (e.g. a nested Cookfile run via
            // `cwd` with no .cookroot above it), the dirs::cache_dir()
            // fallback must still land in the tempdir, never the user's
            // real ~/.cache/cook/cloud.
            .env("XDG_CACHE_HOME", tmp.path().join("xdg"))
            .env_remove("COOK_NO_PRUNE")
            .env_remove("COOK_NO_PUBLISH")
            .env_remove("COOK_CLOUD_API_KEY")
            .envs(&run.env)
            .output()
        {
            Ok(out) => out,
            Err(e) => return Err(format!("{label}: failed to spawn cook: {e}")),
        };
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let code = out.status.code().unwrap_or(-1);
        if code != run.exit {
            return Err(format!(
                "{label}: exit {code}, want {}\n--- output ---\n{combined}",
                run.exit
            ));
        }
        let a = &run.assert;
        for p in &a.exists {
            if !proj.join(p).exists() {
                return Err(format!(
                    "{label}: expected artifact missing: {p}\n--- output ---\n{combined}"
                ));
            }
        }
        for p in &a.absent {
            if proj.join(p).exists() {
                return Err(format!(
                    "{label}: expected path absent, but exists: {p}\n--- output ---\n{combined}"
                ));
            }
        }
        for (p, want) in &a.equals {
            let got = fs::read_to_string(proj.join(p)).map_err(|e| {
                format!("{label}: cannot read {p}: {e}\n--- output ---\n{combined}")
            })?;
            if &got != want {
                return Err(format!(
                    "{label}: {p} content mismatch\n--- got ---\n{got}--- want ---\n{want}"
                ));
            }
        }
        for (p, needle) in &a.contains {
            let got = fs::read_to_string(proj.join(p)).map_err(|e| {
                format!("{label}: cannot read {p}: {e}\n--- output ---\n{combined}")
            })?;
            if !got.contains(needle.as_str()) {
                return Err(format!(
                    "{label}: {p} does not contain {needle:?}\n--- got ---\n{got}"
                ));
            }
        }
        for needle in &a.output_contains {
            if !combined.contains(needle.as_str()) {
                return Err(format!(
                    "{label}: output does not contain {needle:?}\n--- output ---\n{combined}"
                ));
            }
        }
        for p in &a.unchanged {
            let prev = snapshot
                .get(p)
                .ok_or_else(|| format!("{label}: `unchanged` needs a prior run tracking {p}"))?;
            let now = fs::read(proj.join(p)).ok();
            if prev.is_none() || now.is_none() {
                return Err(format!(
                    "{label}: `unchanged` path never produced: {p} ({})\n--- output ---\n{combined}",
                    if prev.is_none() { "prev missing" } else { "now missing" }
                ));
            }
            if prev != &now {
                return Err(format!(
                    "{label}: {p} changed across runs, expected byte-identical (cache hit)\n--- output ---\n{combined}"
                ));
            }
        }
        for p in &a.changed {
            let prev = snapshot
                .get(p)
                .ok_or_else(|| format!("{label}: `changed` needs a prior run tracking {p}"))?;
            let now = fs::read(proj.join(p)).ok();
            if prev.is_none() || now.is_none() {
                return Err(format!(
                    "{label}: `changed` path never produced: {p} ({})\n--- output ---\n{combined}",
                    if prev.is_none() { "prev missing" } else { "now missing" }
                ));
            }
            if prev == &now {
                return Err(format!(
                    "{label}: {p} byte-identical across runs, expected re-execution to change it\n--- output ---\n{combined}"
                ));
            }
        }
        for p in &tracked {
            snapshot.insert(p.clone(), fs::read(proj.join(p)).ok());
        }
    }
    Ok(())
}

#[test]
fn surface_conformance_corpus() {
    let root = corpus_root();
    let filter = std::env::var("COOK_SURFACE_FIXTURE").ok();
    let mut names: Vec<String> = fs::read_dir(&root)
        .expect("read corpus")
        .filter_map(|e| {
            let e = e.ok()?;
            e.file_type()
                .ok()?
                .is_dir()
                .then(|| e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixtures under {}", root.display());

    let mut failures = Vec::new();
    let mut xfailed = Vec::new();
    let mut executed = 0usize;
    for name in &names {
        if let Some(f) = &filter {
            if !name.contains(f.as_str()) {
                continue;
            }
        }
        executed += 1;
        let dir = root.join(name);
        let manifest: Manifest = toml::from_str(
            &fs::read_to_string(dir.join("expect.toml"))
                .unwrap_or_else(|e| panic!("{name}/expect.toml: {e}")),
        )
        .unwrap_or_else(|e| panic!("{name}/expect.toml: {e}"));
        match (run_fixture(&manifest, &dir), &manifest.xfail) {
            (Ok(()), None) => {}
            (Err(e), None) => failures.push(format!("[{name}] {e}")),
            (Err(e), Some(key)) => {
                let first = e.lines().next().unwrap_or("");
                xfailed.push(format!("[{name}] expected-fail ({key}): {first}"));
            }
            (Ok(()), Some(key)) => failures.push(format!(
                "[{name}] XPASS: {key} appears fixed - remove `xfail = \"{key}\"` from expect.toml"
            )),
        }
    }
    if let Some(f) = &filter {
        assert!(
            executed > 0,
            "COOK_SURFACE_FIXTURE={f:?} matched no fixture under {}",
            root.display()
        );
    }
    for x in &xfailed {
        eprintln!("xfail: {x}");
    }
    assert!(
        failures.is_empty(),
        "{} surface fixture(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
