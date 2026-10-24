//! Acceptance fixture for `cook audit`: seven ways a cache key silently misses
//! an input. The fixture project declares every one; each test removes
//! declarations from its Cookfile text and checks the audit reports exactly
//! the matching holes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

type Doc = serde_json::Value;

/// One hole: its name, the single Cookfile line that declares it, and what the
/// audit reports when that line is missing.
struct Hole {
    name: &'static str,
    declaration: &'static str,
    /// (kind, path prefix or env name): a `read` matches a reported path, an `env` the root.
    expect: &'static [(&'static str, &'static str)],
}

const HOLES: &[Hole] = &[
    Hole { name: "undeclared lint config", declaration: "    seal \".lintrc.json\"\n", expect: &[("read", ".lintrc.json")] },
    Hole { name: "undeclared config dir", declaration: "    seal \".lint/**\"\n", expect: &[("read", ".lint/")] },
    Hole { name: "binary asset read by a test", declaration: "    seal \"assets/head.glb\"\n", expect: &[("read", "assets/head.glb")] },
    Hole { name: "manifest exports read by a typecheck", declaration: "    seal \"pkg/package.json\"\n", expect: &[("read", "pkg/package.json")] },
    Hole { name: "lockfile-only dependency change", declaration: "    seal deps\n", expect: &[("read", "node_modules/")] },
    Hole {
        name: "toolchain selected by env",
        declaration: "    seal channel\n",
        expect: &[("env", "TOOL_CHANNEL"), ("read", "~/.tc/")],
    },
    Hole { name: "env var changing output", declaration: "    seal mode\n", expect: &[("env", "APP_MODE")] },
];

/// The fixture project: every declaration present; its baseline holds only the project-wide OS roots (§17.9).
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/audit-seven-holes");

fn require() -> bool {
    std::env::var("COOK_AUDIT_E2E").is_ok_and(|v| v == "require")
}

fn on_path(tool: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|p| p.join(tool).is_file())
}

/// False when node is missing and the host may skip.
fn has_node() -> bool {
    let found = on_path("node");
    assert!(found || !require(), "COOK_AUDIT_E2E=require but node is not on PATH");
    if !found {
        eprintln!("skipping: node not on PATH");
    }
    found
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// Audit a copy of the fixture whose Cookfile lacks `removed` declarations.
/// `None`: this host cannot run the audit (skip). Returns the report's findings.
fn audit_without(removed: &[&Hole]) -> Option<Vec<Doc>> {
    let dir = TempDir::new().unwrap();
    copy_tree(Path::new(FIXTURE), dir.path());
    // Created here, not committed: node_modules is gitignored.
    fs::create_dir_all(dir.path().join("node_modules/dep")).unwrap();
    fs::write(dir.path().join("node_modules/dep/index.js"), "module.exports = 1;\n").unwrap();
    let mut cookfile = fs::read_to_string(dir.path().join("Cookfile")).unwrap();
    for h in removed {
        assert!(cookfile.contains(h.declaration), "{}: declaration {:?} not in the fixture Cookfile", h.name, h.declaration);
        cookfile = cookfile.replacen(h.declaration, "", 1);
    }
    fs::write(dir.path().join("Cookfile"), cookfile).unwrap();

    // Not under /tmp: the audit ignores that wholesale.
    let home = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    fs::create_dir_all(home.path().join(".tc/stable")).unwrap();
    fs::write(home.path().join(".tc/stable/v"), "1\n").unwrap();
    let xdg = TempDir::new().unwrap();

    if !on_path("strace") {
        assert!(!require(), "COOK_AUDIT_E2E=require but strace is not on PATH");
        eprintln!("skipping: strace not on PATH");
        return None;
    }
    let out: Output = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_cook")))
        .args(["audit", "build", "--json"])
        .env("XDG_CACHE_HOME", xdg.path())
        .env("HOME", home.path())
        // Steps inherit the environment; the hole names must be unset.
        .env_remove("NODE_ENV")
        .env_remove("TOOL_CHANNEL")
        .env_remove("APP_MODE")
        .current_dir(dir.path())
        .output()
        .expect("run cook audit");
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if all.contains("unsupported here") {
        assert!(!require(), "COOK_AUDIT_E2E=require but audit is unsupported here: {all}");
        eprintln!("skipping: {all}");
        return None;
    }
    let doc: Doc = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {all}"));
    let units = doc["units"].as_array().unwrap();
    assert!(!units.is_empty() && units.iter().all(|u| u["status"] == "traced"), "every unit must be traced: {doc}");
    Some(doc["findings"].as_array().unwrap().clone())
}

fn paths(f: &Doc) -> Vec<&str> {
    f["paths"].as_array().map(|a| a.iter().filter_map(|p| p.as_str()).collect()).unwrap_or_default()
}

/// Whether `findings` holds the (kind, path prefix | env name) the hole expects.
fn reported(findings: &[Doc], (kind, what): (&str, &str)) -> bool {
    findings.iter().any(|f| {
        f["kind"] == kind && if kind == "env" { f["root"] == what } else { paths(f).iter().any(|p| p.starts_with(what)) }
    })
}

#[test]
fn the_fixed_project_reports_none_of_the_seven() {
    if !has_node() {
        return;
    }
    let Some(findings) = audit_without(&[]) else { return };
    for h in HOLES {
        for e in h.expect {
            assert!(!reported(&findings, *e), "{}: {e:?} reported though declared: {findings:?}", h.name);
        }
    }
    for f in &findings {
        if f["kind"] == "read" || f["kind"] == "absent" {
            assert!(
                !paths(f).iter().any(|p| !p.starts_with('/')),
                "a project-relative or ~/ path is unaccounted for: {f}"
            );
        }
    }
    // Positive control: node's own startup names stay reported, so the recorder ran.
    assert!(findings.iter().any(|f| f["kind"] == "env"), "no env finding at all; did the recorder run? {findings:?}");
}

#[test]
fn each_hole_is_reported_alone_when_its_declaration_is_missing() {
    if !has_node() {
        return;
    }
    for hole in HOLES {
        let Some(findings) = audit_without(&[hole]) else { return };
        for e in hole.expect {
            assert!(reported(&findings, *e), "{}: {e:?} not reported: {findings:?}", hole.name);
        }
        for other in HOLES.iter().filter(|o| o.name != hole.name) {
            for e in other.expect {
                assert!(!reported(&findings, *e), "{}: removing it also reported {} ({e:?}): {findings:?}", hole.name, other.name);
            }
        }
    }
}

#[test]
fn all_seven_are_reported_without_their_declarations() {
    if !has_node() {
        return;
    }
    let all: Vec<&Hole> = HOLES.iter().collect();
    let Some(findings) = audit_without(&all) else { return };
    for h in HOLES {
        for e in h.expect {
            assert!(reported(&findings, *e), "{}: {e:?} not reported: {findings:?}", h.name);
        }
    }
}
