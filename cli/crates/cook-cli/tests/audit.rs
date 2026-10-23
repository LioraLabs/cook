//! End-to-end tests for `cook audit`: units re-run under strace in an
//! isolated overlay of the project, undeclared reads reported, nothing of the
//! real tree or the user cache touched.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const LINT: &str = "recipe lint\n    seal \"src/a.ts\"\n    test { cat .lintrc src/a.ts >/dev/null; test -f missing.cfg || true }\n";

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn project(cookfile: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("Cookfile"), cookfile).unwrap();
    fs::write(dir.path().join(".lintrc"), "rc\n").unwrap();
    fs::write(dir.path().join("src/a.ts"), "ts\n").unwrap();
    dir
}

/// Run `cook audit ...`; `None` means this host cannot run it (skip).
fn audit(dir: &Path, xdg: &Path, args: &[&str]) -> Option<Output> {
    audit_with(dir, xdg, args, &[])
}

fn audit_with(dir: &Path, xdg: &Path, args: &[&str], env: &[(&str, &Path)]) -> Option<Output> {
    // CI sets COOK_AUDIT_E2E=require: a host that cannot run the audit fails.
    let require = std::env::var("COOK_AUDIT_E2E").is_ok_and(|v| v == "require");
    if which_strace().is_none() {
        assert!(!require, "COOK_AUDIT_E2E=require but strace is not on PATH");
        eprintln!("skipping: strace not on PATH");
        return None;
    }
    let out = Command::new(cook_bin())
        .arg("audit")
        .args(args)
        .env("XDG_CACHE_HOME", xdg)
        // Steps inherit the environment; the env tests need this one unset.
        .env_remove("NODE_ENV")
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .expect("run cook audit");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if all.contains("unsupported here") {
        assert!(!require, "COOK_AUDIT_E2E=require but audit is unsupported here: {all}");
        eprintln!("skipping: {all}");
        return None;
    }
    Some(out)
}

fn which_strace() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join("strace"))
        .find(|p| p.is_file())
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn finding<'a>(doc: &'a serde_json::Value, kind: &str, root: &str) -> Option<&'a serde_json::Value> {
    doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["recipe"] == "lint" && f["kind"] == kind && f["root"] == root)
}

#[test]
fn undeclared_read_and_absent_probe_are_reported() {
    let dir = project(LINT);
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    assert_eq!(out.status.code(), Some(1), "findings remain -> exit 1");
    let doc = json(&out);
    assert_eq!(doc["recipe"], "lint");
    assert!(finding(&doc, "read", ".lintrc").is_some(), "{doc}");
    assert!(finding(&doc, "absent", "missing.cfg").is_some(), "{doc}");
    for f in doc["findings"].as_array().unwrap() {
        assert!(
            !f["paths"].as_array().unwrap().iter().any(|p| p == "src/a.ts"),
            "sealed input is declared, not a finding: {doc}"
        );
    }
    let traced = doc["units"].as_array().unwrap().iter().filter(|u| u["status"] == "traced").count();
    assert_eq!(traced, 1, "{doc}");
}

#[test]
fn sealing_the_read_removes_its_finding() {
    let dir = project(&LINT.replace("seal \"src/a.ts\"", "seal \"src/a.ts\"\n    seal \".lintrc\""));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    assert!(finding(&doc, "read", ".lintrc").is_none(), "{doc}");
}

fn snapshot(root: &Path) -> (BTreeMap<PathBuf, Vec<u8>>, Vec<PathBuf>) {
    let (mut files, mut dirs) = (BTreeMap::new(), Vec::new());
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if p.is_dir() {
                dirs.push(rel);
                stack.push(p);
            } else {
                files.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    dirs.sort();
    (files, dirs)
}

#[test]
fn audit_leaves_the_tree_and_the_user_cache_untouched() {
    let dir = project(LINT);
    let xdg = TempDir::new().unwrap();
    let before = snapshot(dir.path());
    let Some(_) = audit(dir.path(), xdg.path(), &["lint"]) else { return };
    assert!(before == snapshot(dir.path()), "project tree changed");
    assert!(!dir.path().join(".cook").exists());
    assert!(!xdg.path().join("cook/cloud").exists(), "user artifact store was created");
    let sandbox = xdg.path().join("cook/sandbox");
    assert!(
        !sandbox.exists() || fs::read_dir(&sandbox).unwrap().next().is_none(),
        "leftover scratch under {}",
        sandbox.display()
    );
}

#[test]
fn a_declared_output_that_is_also_an_input_survives_the_audit() {
    let dir = project("recipe fmt\n    gather \"f.txt\"\n    cook \"f.txt\" { tr a-z A-Z < $<in> > f.tmp && mv f.tmp $<out> }\n");
    fs::write(dir.path().join("f.txt"), "v1\n").unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["fmt", "--json"]) else { return };
    let doc = json(&out);
    let units = doc["units"].as_array().unwrap();
    assert!(!units.is_empty() && units.iter().all(|u| u["status"] == "traced"), "{doc}");
    assert_eq!(fs::read_to_string(dir.path().join("f.txt")).unwrap(), "v1\n");
}

#[test]
fn human_output_lists_findings_and_a_summary() {
    let dir = project(LINT);
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint"]) else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.lines().any(|l| l.contains("lint") && l.contains("read") && l.contains(".lintrc")),
        "{text}"
    );
    assert!(text.lines().last().unwrap().starts_with("audit: 1 unit(s) traced"), "{text}");
}

#[test]
fn an_output_declared_outside_the_project_is_not_deleted() {
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("keep.txt");
    fs::write(&target, "original\n").unwrap();
    let t = target.display();
    let dir = project(&format!(
        "recipe far\n    seal \"src/a.ts\"\n    cook \"{t}\" {{ echo new > {t} }}\n"
    ));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["far", "--json"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("outside the project"), "{err}");
    // Refused before the populate run, which would have written it.
    assert_eq!(fs::read_to_string(&target).unwrap(), "original\n", "{err}");
}

#[test]
fn an_output_reached_through_a_symlink_out_of_the_project_is_not_deleted() {
    let outside = TempDir::new().unwrap();
    fs::create_dir(outside.path().join("sub")).unwrap();
    let target = outside.path().join("sub/f.txt");
    fs::write(&target, "original\n").unwrap();
    let dir = project(
        "recipe far\n    seal \"src/a.ts\"\n    cook \"link/sub/f.txt\" { test -f link/sub/f.txt || echo new > link/sub/f.txt }\n",
    );
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["far", "--json"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "original\n", "{err}");
}

#[test]
fn an_output_declared_through_a_symlink_out_of_the_project_is_refused_before_anything_runs() {
    let base = TempDir::new().unwrap();
    let (proj, outside) = (base.path().join("proj"), base.path().join("outside"));
    fs::create_dir_all(outside.join("gen")).unwrap();
    fs::create_dir_all(&proj).unwrap();
    fs::write(outside.join("gen/precious"), "PRECIOUS\n").unwrap();
    std::os::unix::fs::symlink("../outside/gen", proj.join("gen")).unwrap();
    fs::write(
        proj.join("Cookfile"),
        "cook.recipe(\"build\", {}, function() cook.add_unit({ output = \"gen/new\", command = \"echo x > gen/new\" }) end)\n",
    )
    .unwrap();
    let before = snapshot(&outside);
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(&proj, xdg.path(), &["build"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(before == snapshot(&outside), "the outside tree changed: {err}");
    assert!(!outside.join(".cook").exists());
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("outside the project"), "{err}");
}

#[test]
fn a_single_deep_read_shows_its_path_and_an_empty_audit_says_so() {
    let dir = project("recipe deep\n    seal \"src/a.ts\"\n    test { cat src/a.ts src/b.ts >/dev/null }\n");
    fs::write(dir.path().join("src/b.ts"), "b\n").unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["deep"]) else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|l| l == "  (src/b.ts)"), "{text}");

    let dir = project("recipe none\n    test { true }\n");
    let Some(out) = audit(dir.path(), xdg.path(), &["none"]) else { return };
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("audit: nothing to audit in none (no cacheable command units)"), "{err}");
}

#[test]
fn a_member_symlinked_out_of_the_project_is_refused_before_anything_runs() {
    let base = TempDir::new().unwrap();
    let (proj, outside) = (base.path().join("proj"), base.path().join("outside"));
    fs::create_dir_all(outside.join("gen")).unwrap();
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        outside.join("Cookfile"),
        // The register-time write must not happen either.
        "cook.recipe(\"generated\", {}, function() fs.write(\"registered\", \"x\"); cook.add_unit({ output = \"gen/**\", command = \"mkdir -p gen && echo x > gen/new\" }) end)\n",
    )
    .unwrap();
    fs::write(outside.join("gen/precious"), "PRECIOUS\n").unwrap();
    std::os::unix::fs::symlink("../outside", proj.join("api")).unwrap();
    fs::write(proj.join("Cookfile"), "import api ./api\n\nrecipe build: api.generated\n").unwrap();
    let before = snapshot(&outside);
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(&proj, xdg.path(), &["build"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(before == snapshot(&outside), "the member's real directory changed: {err}");
    assert!(!outside.join(".cook").exists());
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("outside the project"), "{err}");
}

#[test]
fn a_materializer_never_touches_the_user_artifact_store() {
    let dir = project(
        "cook.materialize(\"graph\", {}, function() return true end)\n\
         cook.recipe(\"build\", {}, function() cook.add_unit({ output = \"out.txt\", command = \"cat .lintrc > out.txt\" }) end)\n",
    );
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["build", "--json"]) else { return };
    let doc = json(&out);
    assert!(doc["units"].as_array().unwrap().iter().all(|u| u["status"] == "traced"), "{doc}");
    assert!(!xdg.path().join("cook/cloud").exists(), "user artifact store was created");
}

#[test]
fn the_invoking_shells_pwd_is_not_a_finding() {
    let dir = project(LINT);
    // A PWD naming the working dir by another spelling survives the shell's
    // own check (same inode), so the traced shell would stat that spelling.
    let elsewhere = dir.path().join("elsewhere");
    std::os::unix::fs::symlink(".", &elsewhere).unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit_with(dir.path(), xdg.path(), &["lint", "--json"], &[("PWD", &elsewhere), ("OLDPWD", &elsewhere)]) else {
        return;
    };
    let doc = json(&out);
    assert!(!doc["findings"].to_string().contains("elsewhere"), "{doc}");
}

#[test]
fn a_daemon_a_unit_starts_does_not_outlive_the_audit() {
    let marker = format!("{}.{}", std::process::id(), 7);
    // Only the populate run starts it: `strace -f` would wait on a daemon
    // started under it, so the traced re-run (which finds `started`) must not.
    let dir = project(&format!(
        "recipe bg\n    seal \"src/a.ts\"\n    cook \"o.txt\" {{ if [ ! -e started ]; then touch started; (setsid sleep 30{marker} >/dev/null 2>&1 &); fi; echo x > o.txt }}\n"
    ));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["bg", "--json"]) else { return };
    let doc = json(&out);
    assert!(doc["units"].as_array().unwrap().iter().all(|u| u["status"] == "traced"), "{doc}");
    let alive: Vec<i32> = fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter(|e| {
            fs::read(e.path().join("cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains(&format!("30{marker}")))
        })
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect();
    for pid in &alive {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    assert!(alive.is_empty(), "a daemon started by the audited unit is still running");
}

#[test]
fn a_cook_dir_symlinked_out_of_the_project_is_refused_before_anything_runs() {
    let base = TempDir::new().unwrap();
    let (proj, state) = (base.path().join("proj"), base.path().join("state"));
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&proj).unwrap();
    std::os::unix::fs::symlink("../state", proj.join(".cook")).unwrap();
    fs::write(
        proj.join("Cookfile"),
        "cook.recipe(\"build\", {}, function() cook.add_unit({ output = \"o.txt\", command = \"echo x > o.txt\" }) end)\n",
    )
    .unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(&proj, xdg.path(), &["build"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(fs::read_dir(&state).unwrap().next().is_none(), "cook wrote its state outside the project: {err}");
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("`.cook` resolves outside the project"), "{err}");
}

#[test]
fn a_members_cook_dir_symlinked_out_of_the_project_is_refused_before_anything_runs() {
    let base = TempDir::new().unwrap();
    let (proj, state) = (base.path().join("proj"), base.path().join("state"));
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(proj.join("api")).unwrap();
    std::os::unix::fs::symlink("../../state", proj.join("api/.cook")).unwrap();
    fs::write(
        proj.join("api/Cookfile"),
        "cook.recipe(\"generated\", {}, function() cook.add_unit({ output = \"o.txt\", command = \"echo x > o.txt\" }) end)\n",
    )
    .unwrap();
    fs::write(proj.join("Cookfile"), "import api ./api\n\nrecipe build: api.generated\n").unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(&proj, xdg.path(), &["build"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(fs::read_dir(&state).unwrap().next().is_none(), "cook wrote its state outside the project: {err}");
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("`.cook` resolves outside the project"), "{err}");
}

#[test]
fn the_entry_cookfiles_cook_dir_below_a_cookroot_is_checked_too() {
    let base = TempDir::new().unwrap();
    let (proj, state) = (base.path().join("proj"), base.path().join("state"));
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(proj.join("sub")).unwrap();
    fs::write(proj.join(".cookroot"), "").unwrap();
    std::os::unix::fs::symlink("../../state", proj.join("sub/.cook")).unwrap();
    fs::write(
        proj.join("sub/Cookfile"),
        "cook.recipe(\"build\", {}, function() cook.add_unit({ output = \"o.txt\", command = \"echo x > o.txt\" }) end)\n",
    )
    .unwrap();
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(&proj.join("sub"), xdg.path(), &["build"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(fs::read_dir(&state).unwrap().next().is_none(), "cook wrote its state outside the project: {err}");
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("`.cook` resolves outside the project"), "{err}");
}

#[test]
fn a_unit_that_runs_cook_does_not_publish_to_the_user_store() {
    let dir = project(&format!(
        "cook.recipe(\"inner\", {{}}, function() cook.add_unit({{ output = \"i.txt\", command = \"echo i > i.txt\" }}) end)\n\
         cook.recipe(\"build\", {{}}, function() cook.add_unit({{ output = \"o.txt\", command = \"{} inner && cp i.txt o.txt\" }}) end)\n",
        cook_bin().display()
    ));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["build", "--json"]) else { return };
    let doc = json(&out);
    assert!(doc["units"].as_array().unwrap().iter().all(|u| u["status"] == "traced"), "{doc}");
    let store = xdg.path().join("cook/cloud");
    let published: Vec<_> = if store.exists() { snapshot(&store).0.into_keys().collect() } else { Vec::new() };
    assert!(published.is_empty(), "the nested cook published to the user store: {published:?}");
}

#[test]
fn sigterm_to_the_audit_ends_the_whole_tree_and_cleans_its_scratch() {
    let marker = format!("{}.{}", std::process::id(), 9);
    let dir = project(&format!(
        "cook.recipe(\"build\", {{}}, function() cook.add_unit({{ output = \"o.txt\", command = \"sleep 40{marker}; echo x > o.txt\" }}) end)\n"
    ));
    let xdg = TempDir::new().unwrap();
    // Probes support (isolation happens before the recipe is looked up).
    if audit(dir.path(), xdg.path(), &["no-such-recipe"]).is_none() {
        return;
    }
    let signal = |pid: i32, sig: &str| {
        let _ = Command::new("kill").args([sig, &pid.to_string()]).status();
    };
    let sleeping = || -> Vec<i32> {
        fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter(|e| {
                fs::read(e.path().join("cmdline"))
                    // `"sleep\040{marker}"` is `sleep`, NUL, then `40<marker>`: /proc cmdline separates args with NUL.
                    .is_ok_and(|c| String::from_utf8_lossy(&c).starts_with(&format!("sleep\040{marker}")))
            })
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect()
    };
    let mut child = Command::new(cook_bin())
        .args(["audit", "build"])
        .env("XDG_CACHE_HOME", xdg.path())
        .current_dir(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while sleeping().is_empty() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the unit never started");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Only the audit process, as a CI timeout sends it.
    signal(child.id() as i32, "-TERM");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            sleeping().into_iter().for_each(|pid| signal(pid, "-KILL"));
            panic!("SIGTERM did not end the audit");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(128 + 15));
    let left = sleeping();
    left.iter().for_each(|&pid| signal(pid, "-KILL"));
    assert!(left.is_empty(), "the unit's process outlived the audit");
    let sandbox = xdg.path().join("cook/sandbox");
    assert!(
        fs::read_dir(&sandbox).map_or(true, |mut d| d.next().is_none()),
        "leftover scratch under {}",
        sandbox.display()
    );
}

// --- environment variables a step reads that nothing keys -------------------

/// The reader must be node: it asks libc `getenv`, which the recorder sees (a
/// shell's `$X` or python's `os.environ` read `environ` directly).
const NODE_READ: &str = "node -e 'if (process.env.NODE_ENV === \"production\") {}'";

/// False when node is missing and the host is allowed to skip.
fn has_node() -> bool {
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|p| p.join("node").is_file());
    let require = std::env::var("COOK_AUDIT_E2E").is_ok_and(|v| v == "require");
    assert!(found || !require, "COOK_AUDIT_E2E=require but node is not on PATH");
    if !found {
        eprintln!("skipping: node not on PATH");
    }
    found
}

fn env_roots(doc: &serde_json::Value) -> Vec<String> {
    doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "env")
        .map(|f| f["root"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn an_unkeyed_environment_read_is_reported_by_name() {
    if !has_node() {
        return;
    }
    let dir = project(&format!("recipe lint\n    seal \"src/a.ts\"\n    test {{ {NODE_READ} }}\n"));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    let f = finding(&doc, "env", "NODE_ENV").unwrap_or_else(|| panic!("{doc}"));
    assert_eq!(f["count"], 1);
    assert_eq!(f["paths"], serde_json::json!([]));
    let text = String::from_utf8_lossy(&audit(dir.path(), xdg.path(), &["lint"]).unwrap().stdout).into_owned();
    assert!(text.lines().any(|l| l.contains("lint  env  NODE_ENV") && l.contains("(unset)")), "{text}");
}

#[test]
fn processes_that_read_no_variable_report_none() {
    let dir = project("recipe quiet\n    seal \"src/a.ts\"\n    test { true }\n    test { cat src/a.ts >/dev/null }\n");
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["quiet", "--json"]) else { return };
    let doc = json(&out);
    assert_eq!(env_roots(&doc), Vec::<String>::new(), "the harness (sh, strace) must add no names: {doc}");
    // Canary for the recorder-path match: were it wrong, every program would be named.
    assert_eq!(unobserved_roots(&doc), Vec::<String>::new(), "{doc}");
}

#[test]
fn a_sealed_probes_literal_getenv_keys_the_name() {
    if !has_node() {
        return;
    }
    let dir = project(&format!(
        "probe mode\n    >{{\n        return {{ v = os.getenv(\"NODE_ENV\") or \"\" }}\n    }}\n\nrecipe lint\n    seal \"src/a.ts\"\n    seal mode\n    test {{ {NODE_READ} }}\n"
    ));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    assert!(doc["units"].as_array().unwrap().iter().all(|u| u["status"] == "traced"), "{doc}");
    assert!(finding(&doc, "env", "NODE_ENV").is_none(), "{doc}");
}

/// A documented false positive: the audit cannot attribute a variable's
/// value to the environment name config read it from, so the name stays reported.
#[test]
fn a_variable_assigned_from_the_environment_does_not_key_the_name() {
    if !has_node() {
        return;
    }
    let dir = project(&format!(
        "config\n    var.mode = host.env(\"NODE_ENV\", \"dev\")\n\nrecipe lint\n    seal \"src/a.ts\"\n    test {{ {NODE_READ}; echo $<mode> >/dev/null }}\n"
    ));
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    assert!(finding(&doc, "env", "NODE_ENV").is_some(), "{doc}");
}

#[test]
fn a_variable_that_shares_the_names_spelling_does_not_key_it() {
    if !has_node() {
        return;
    }
    let dir = project(
        "config\n    var.RUSTFLAGS = \"-C x\"\n\nrecipe lint\n    seal \"src/a.ts\"\n    test { node -e 'process.env.RUSTFLAGS'; echo $<RUSTFLAGS> >/dev/null }\n",
    );
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    assert!(finding(&doc, "env", "RUSTFLAGS").is_some(), "{doc}");
}

#[test]
fn locale_and_home_queries_are_not_findings() {
    if !has_node() {
        return;
    }
    let dir = project("recipe lint\n    seal \"src/a.ts\"\n    test { node -e 'process.env.LANG; process.env.LC_ALL; process.env.HOME; process.env.LD_LIBRARY_PATH; process.env.TERM' }\n");
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    // Node queries its own NODE_* names too; those are real findings.
    let roots = env_roots(&doc);
    for ignored in ["LANG", "LC_ALL", "HOME", "LD_LIBRARY_PATH", "TERM"] {
        assert!(!roots.iter().any(|r| r == ignored), "{ignored} is ignored: {doc}");
    }
    assert!(roots.iter().any(|r| r == "NODE_OPTIONS"), "the recorder saw nothing at all: {doc}");
}

// --- statically linked programs ----------------------------------------------

/// Builds `tools/stat` (a static no-op) in the project; false when this host
/// cannot link statically and is allowed to skip.
fn build_static(dir: &Path) -> bool {
    let require = std::env::var("COOK_AUDIT_E2E").is_ok_and(|v| v == "require");
    fs::create_dir_all(dir.join("tools")).unwrap();
    let c = dir.join("tools/stat.c");
    fs::write(&c, "int main(void) { return 0; }\n").unwrap();
    let ok = Command::new("cc")
        .args(["-static", "-o"])
        .arg(dir.join("tools/stat"))
        .arg(&c)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok || !require, "COOK_AUDIT_E2E=require but `cc -static` failed");
    if !ok {
        eprintln!("skipping: cc -static unavailable");
    }
    fs::remove_file(&c).unwrap();
    ok
}

fn unobserved_roots(doc: &serde_json::Value) -> Vec<String> {
    doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "env-unobserved")
        .map(|f| f["root"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_statically_linked_program_is_named_as_env_not_observed() {
    let dir = project("recipe lint\n    seal \"src/a.ts\" \"tools/stat\"\n    test { ./tools/stat }\n");
    if !build_static(dir.path()) {
        return;
    }
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let doc = json(&out);
    let f = finding(&doc, "env-unobserved", "tools/stat").unwrap_or_else(|| panic!("{doc}"));
    assert_eq!(f["count"], 1);
    assert_eq!(f["paths"], serde_json::json!([]));
    let text = String::from_utf8_lossy(&audit(dir.path(), xdg.path(), &["lint"]).unwrap().stdout).into_owned();
    assert!(text.lines().any(|l| l.contains("lint  env-unobserved  tools/stat") && l.contains("env not observed")), "{text}");
}

#[test]
fn dynamic_programs_are_not_named_unobserved() {
    let dir = project("recipe dyn\n    seal \"src/a.ts\"\n    test { true; cat src/a.ts >/dev/null }\n");
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["dyn", "--json"]) else { return };
    assert_eq!(unobserved_roots(&json(&out)), Vec::<String>::new());
}

/// Audits one `test { <cmd> }` recipe and returns the env-unobserved roots.
fn unobserved_by(cmd: &str) -> Option<Vec<String>> {
    let dir = project(&format!("recipe scrub\n    seal \"src/a.ts\"\n    test {{ {cmd} }}\n"));
    let xdg = TempDir::new().unwrap();
    let out = audit(dir.path(), xdg.path(), &["scrub", "--json"])?;
    Some(unobserved_roots(&json(&out)))
}

#[test]
fn a_program_started_with_a_scrubbed_environment_is_named() {
    if !has_node() {
        return;
    }
    let Some(roots) = unobserved_by("env -i PATH=\"$PATH\" node -e 1") else { return };
    assert!(roots.iter().any(|r| r.ends_with("/node")), "{roots:?}");
}

#[test]
fn a_child_spawned_with_an_empty_env_is_named() {
    if !has_node() {
        return;
    }
    let Some(roots) = unobserved_by("node -e 'require(\"child_process\").spawnSync(\"true\",{env:{}})'") else { return };
    assert!(roots.iter().any(|r| r.ends_with("/true")), "{roots:?}");
}

#[test]
fn a_child_spawned_with_the_inherited_env_is_not_named() {
    if !has_node() {
        return;
    }
    let Some(roots) = unobserved_by("node -e 'require(\"child_process\").spawnSync(\"true\")'") else { return };
    assert_eq!(roots, Vec::<String>::new());
}

fn paths_of(doc: &serde_json::Value, recipe: &str) -> Vec<String> {
    doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["recipe"] == recipe)
        .flat_map(|f| f["paths"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()))
        .collect()
}

/// A project whose recipe reads under `node_modules` at the root, in a package
/// and (absent) under `src`, plus `.lintrc`; `seal` is the recipe's seal line.
fn node_modules_project(seal: &str) -> TempDir {
    let dir = project(&format!(
        "files deps\n    \"lock\"\n    covers \"**/node_modules\"\n\nrecipe lint\n    {seal}\n    test {{ cat node_modules/a/x pkg/node_modules/b/y .lintrc >/dev/null; test -e src/node_modules || true }}\n"
    ));
    for f in ["lock", "node_modules/a/x", "pkg/node_modules/b/y"] {
        fs::create_dir_all(dir.path().join(f).parent().unwrap()).unwrap();
        fs::write(dir.path().join(f), "x\n").unwrap();
    }
    dir
}

#[test]
fn a_sealed_covers_root_removes_its_reads_and_keeps_project_files() {
    let dir = node_modules_project("seal deps");
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let paths = paths_of(&json(&out), "lint");
    assert!(!paths.iter().any(|p| p.contains("node_modules")), "{paths:?}");
    assert!(paths.iter().any(|p| p == ".lintrc"), "{paths:?}");
}

#[test]
fn an_unsealed_covers_root_covers_nothing() {
    let dir = node_modules_project("seal \"src/a.ts\"");
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["lint", "--json"]) else { return };
    let paths = paths_of(&json(&out), "lint");
    for want in ["node_modules/a/x", "pkg/node_modules/b/y", "src/node_modules"] {
        assert!(paths.iter().any(|p| p == want), "{want} missing: {paths:?}");
    }
}

#[test]
fn a_toolchain_root_is_covered_by_a_sealed_version_probe_not_by_the_tool_hash() {
    // Not under /tmp: the audit ignores that wholesale.
    let home = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let h = home.path();
    fs::create_dir_all(h.join("tc/lib")).unwrap();
    fs::create_dir_all(h.join("bin")).unwrap();
    fs::write(h.join("tc/lib/data"), "d\n").unwrap();
    let tool = h.join("bin/tool");
    fs::write(&tool, "#!/bin/sh\ncat \"$HOME/tc/lib/data\"\n").unwrap();
    fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let dir = project(
        "tools t\n    tool\n\nprobe v\n    covers \"~/tc\"\n    { tool >/dev/null; echo 1 }\n\n\
         recipe x\n    seal t\n    test { tool >/dev/null }\n\n\
         recipe y\n    seal v\n    test { tool >/dev/null }\n",
    );
    let xdg = TempDir::new().unwrap();
    let path = format!("{}:{}", h.join("bin").display(), std::env::var("PATH").unwrap());
    let env: [(&str, &Path); 2] = [("HOME", h), ("PATH", Path::new(&path))];
    let Some(out) = audit_with(dir.path(), xdg.path(), &["x", "--json"], &env) else { return };
    let (doc, x) = (json(&out), paths_of(&json(&out), "x"));
    assert!(x.iter().any(|p| p.starts_with("~/tc")), "tool hash must not cover the root: {doc}");
    let Some(out) = audit_with(dir.path(), xdg.path(), &["y", "--json"], &env) else { return };
    let (doc, y) = (json(&out), paths_of(&json(&out), "y"));
    assert!(!y.iter().any(|p| p.starts_with("~/tc")), "{doc}");
}

#[test]
fn a_glob_in_a_leading_component_is_refused_as_its_literal_root() {
    let dir = project("probe v\n    covers \"/h*\"\n    { echo 1 }\n\nrecipe x\n    seal v\n    test { cat .lintrc }\n");
    let xdg = TempDir::new().unwrap();
    let Some(out) = audit(dir.path(), xdg.path(), &["x", "--json"]) else { return };
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("'/h*' is too broad"), "{err}");
}

#[test]
fn a_covers_root_that_is_home_errors_its_unit_at_audit_and_breaks_nothing_else() {
    // Not under /tmp: the audit ignores that wholesale.
    let home = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let h = home.path();
    let dir = project(&format!(
        "probe v\n    covers \"{}\"\n    {{ echo 1 }}\n\nrecipe x\n    seal v\n    test {{ cat .lintrc }}\n\nrecipe y\n    seal \"src/a.ts\"\n    test {{ cat src/a.ts }}\n",
        h.display()
    ));
    let xdg = TempDir::new().unwrap();
    let env: [(&str, &Path); 1] = [("HOME", h)];
    let run = Command::new(cook_bin()).arg("x").env("XDG_CACHE_HOME", xdg.path()).env("HOME", h).current_dir(dir.path()).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let Some(out) = audit_with(dir.path(), xdg.path(), &["x", "--json"], &env) else { return };
    let doc = json(&out);
    let units = doc["units"].as_array().unwrap();
    assert!(!units.is_empty() && units.iter().all(|u| u["status"] == "error" && u["detail"].as_str().unwrap().contains("too broad")), "{doc}");
    let Some(out) = audit_with(dir.path(), xdg.path(), &["y", "--json"], &env) else { return };
    let doc = json(&out);
    let units = doc["units"].as_array().unwrap();
    assert!(!units.is_empty() && units.iter().all(|u| u["status"] == "traced"), "{doc}");
}

#[test]
fn a_unit_whose_coverage_errors_still_reports_its_environment_reads() {
    if !has_node() {
        return;
    }
    let home = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let h = home.path();
    let dir = project(&format!("probe v\n    covers \"{}\"\n    {{ echo 1 }}\n\nrecipe x\n    seal v\n    test {{ {NODE_READ} }}\n", h.display()));
    let xdg = TempDir::new().unwrap();
    let env: [(&str, &Path); 1] = [("HOME", h)];
    let Some(out) = audit_with(dir.path(), xdg.path(), &["x", "--json"], &env) else { return };
    let doc = json(&out);
    let units = doc["units"].as_array().unwrap();
    assert!(!units.is_empty() && units.iter().all(|u| u["status"] == "error"), "{doc}");
    assert!(env_roots(&doc).iter().any(|r| r == "NODE_ENV"), "{doc}");
}
