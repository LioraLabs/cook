//! CS-0093 / §17.7 end-to-end: when a recipe's declared output set shrinks
//! between runs, the orphaned output Cook previously wrote is swept — guarded
//! by a content-hash check (a user-modified orphan is kept) and disabled by
//! `--no-prune`.

use std::fs;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // /target/debug/deps  →  /target/debug
    path.pop(); // /target/debug       →  /target
    path.push("cook");
    if !path.exists() {
        panic!(
            "cook binary not found at {} — run `cargo build --bin cook` first",
            path.display()
        );
    }
    path
}

/// Point the cook cache at a private directory so test runs sharing the same
/// source content / command hash do not collide on artifact keys in the
/// system-wide local backend (`~/.cache/cook/cloud`).
fn write_isolated_cache_config(wd: &std::path::Path, cache_dir: &std::path::Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

/// A recipe whose single unit declares `outputs`, producing one file per
/// declared output from `src.txt`.
fn write_cookfile(wd: &std::path::Path, outputs: &[&str]) {
    let outs = outputs
        .iter()
        .map(|o| format!("{o:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let copies = outputs
        .iter()
        .map(|o| format!("cp src.txt {o}"))
        .collect::<Vec<_>>()
        .join(" && ");
    fs::write(
        wd.join("Cookfile"),
        format!(
            "recipe build\n    >>{{\n        cook.add_unit({{\n            inputs  = {{ \"src.txt\" }},\n            outputs = {{ {outs} }},\n            command = \"mkdir -p build && {copies}\",\n        }})\n    }}\n"
        ),
    )
    .unwrap();
}

fn run_build(wd: &std::path::Path, extra_args: &[&str]) -> std::process::Output {
    let mut cmd = std::process::Command::new(cook_binary());
    cmd.args(extra_args).arg("+build").current_dir(wd);
    let out = cmd.output().expect("cook invocation");
    assert!(
        out.status.success(),
        "cook +build {extra_args:?} failed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn shrunk_output_set_sweeps_orphan() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_isolated_cache_config(wd, cache_tmp.path());
    fs::write(wd.join("src.txt"), b"payload").unwrap();

    // Run 1: declare two outputs.
    write_cookfile(wd, &["build/a.txt", "build/b.txt"]);
    run_build(wd, &[]);
    assert!(wd.join("build/a.txt").exists());
    assert!(wd.join("build/b.txt").exists());

    // Run 2: the output set shrinks to just a.txt.
    write_cookfile(wd, &["build/a.txt"]);
    run_build(wd, &[]);

    assert!(wd.join("build/a.txt").exists(), "live output retained");
    assert!(
        !wd.join("build/b.txt").exists(),
        "orphaned output b.txt must be swept (§17.7)"
    );
}

#[test]
fn modified_orphan_is_kept() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_isolated_cache_config(wd, cache_tmp.path());
    fs::write(wd.join("src.txt"), b"payload").unwrap();

    write_cookfile(wd, &["build/a.txt", "build/b.txt"]);
    run_build(wd, &[]);

    // User edits b.txt after Cook wrote it.
    fs::write(wd.join("build/b.txt"), b"HAND EDITED").unwrap();

    write_cookfile(wd, &["build/a.txt"]);
    run_build(wd, &[]);

    assert!(
        wd.join("build/b.txt").exists(),
        "user-modified orphan must be kept (hash guard, §17.7)"
    );
    assert_eq!(fs::read(wd.join("build/b.txt")).unwrap(), b"HAND EDITED");
}

#[test]
fn no_prune_retains_orphan() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_isolated_cache_config(wd, cache_tmp.path());
    fs::write(wd.join("src.txt"), b"payload").unwrap();

    write_cookfile(wd, &["build/a.txt", "build/b.txt"]);
    run_build(wd, &[]);

    write_cookfile(wd, &["build/a.txt"]);
    run_build(wd, &["--no-prune"]);

    assert!(
        wd.join("build/b.txt").exists(),
        "--no-prune must retain the orphan (§17.7 opt-out)"
    );
}
