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
