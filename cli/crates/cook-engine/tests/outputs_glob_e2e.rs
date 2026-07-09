//! CS-0085 end-to-end: a recipe with `outputs = { "build/**" }` caches on
//! cold run, is a skip on hot rerun with no source changes, and restores
//! outputs from cache when removed from disk.
//!
//! §17.6: "On a subsequent cache check for the same unit, the implementation
//! MUST use the recorded `StepEntry.outputs` directly for the disk-integrity
//! walk and for `try_restore`."

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

fn write_cookfile(wd: &std::path::Path) {
    fs::write(
        wd.join("Cookfile"),
        r#"recipe build
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/**" },
            command = "mkdir -p build && cp src.c build/a.out && date > build/stamp",
        })
"#,
    )
    .unwrap();
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

#[test]
fn cold_then_hot_then_restore_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_cookfile(wd);
    write_isolated_cache_config(wd, cache_tmp.path());
    fs::write(wd.join("src.c"), b"source 1").unwrap();

    // Cold run: build/ does not exist before cook starts.
    let out1 = std::process::Command::new(cook_binary())
        .arg("+build")
        .current_dir(wd)
        .output()
        .expect("first cook invocation");
    assert!(
        out1.status.success(),
        "cold run failed. stderr:\n{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(wd.join("build/a.out").exists(), "cold run produced build/a.out");
    assert!(wd.join("build/stamp").exists(), "cold run produced build/stamp");
    let stamp1 = fs::read(wd.join("build/stamp")).unwrap();

    // Hot rerun: no source changes -> cache hit, command does not run.
    // Detection: compare stamp bytes (the command writes `date` output;
    // if the command ran again, stamp would change).
    std::thread::sleep(std::time::Duration::from_secs(1)); // ensure mtime would differ if cmd ran
    let out2 = std::process::Command::new(cook_binary())
        .arg("+build")
        .current_dir(wd)
        .output()
        .expect("second cook invocation");
    assert!(
        out2.status.success(),
        "hot rerun failed. stderr:\n{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stamp2 = fs::read(wd.join("build/stamp")).unwrap();
    assert_eq!(
        stamp1, stamp2,
        "hot rerun should be a cache hit; command must not have run (stamp bytes changed)"
    );

    // Restore: remove build/, rerun, expect files restored from cache without rerun.
    fs::remove_dir_all(wd.join("build")).unwrap();
    let out3 = std::process::Command::new(cook_binary())
        .arg("+build")
        .current_dir(wd)
        .output()
        .expect("third cook invocation (restore)");
    assert!(
        out3.status.success(),
        "restore run failed. stderr:\n{}",
        String::from_utf8_lossy(&out3.stderr)
    );
    assert!(
        wd.join("build/a.out").exists(),
        "restore brought back build/a.out"
    );
    assert!(
        wd.join("build/stamp").exists(),
        "restore brought back build/stamp"
    );
    let stamp3 = fs::read(wd.join("build/stamp")).unwrap();
    assert_eq!(
        stamp1, stamp3,
        "restore should bring back the EXACT bytes from the cold run, \
         not regenerate by running the command again"
    );
}
