//! COOK-187 end-to-end: probe-value references in NATIVE cook-step bodies
//! (Standard §22.5.7 / §8.4). Regression: luagen emitted the command as a
//! `function() ... end` closure which cook.add_unit coerced to "" — the unit
//! "ran" 0.00s, produced nothing, exited 0.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // .../target/debug/deps -> .../target/debug
    path.pop();
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build -p cook-cli` first",
        path.display()
    );
    path
}

/// Private cache dir so runs never touch ~/.cache/cook/cloud (shared).
fn write_cloud_toml(wd: &Path, cache_dir: &Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

fn run_cook(wd: &Path, target: &str) -> std::process::Output {
    Command::new(cook_binary())
        .arg(target)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

#[test]
fn probe_ref_in_native_cook_body_substitutes_value() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe cook187a:val
    { printf 'zesty-e2e-value' }

recipe withprobe
    cook "out/b.txt" { echo "$<cook187a:val>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "withprobe");
    assert!(
        out.status.success(),
        "cook withprobe failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let b = wd.join("out/b.txt");
    assert!(b.exists(), "out/b.txt was not produced — probe-in-body silently no-op'd");
    let content = fs::read_to_string(&b).unwrap();
    assert_eq!(
        content.trim(),
        "zesty-e2e-value",
        "probe value must be substituted into the body; got: {content:?}"
    );
}

#[test]
fn probe_ref_mixed_with_dep_output_ref_in_one_body() {
    // Spec: a body containing BOTH a dep ref and a probe ref failed wholesale
    // (the closure carried the whole command). Both must substitute.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe cook187b:val
    { printf 'probe-part' }

recipe gen
    cook "out/a.txt" { printf 'dep-part' > $<out> }

recipe combined: gen
    cook "out/c.txt" { printf '%s %s' "$(cat $<gen>)" "$<cook187b:val>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "combined");
    assert!(
        out.status.success(),
        "cook combined failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let c = wd.join("out/c.txt");
    assert!(c.exists(), "out/c.txt was not produced");
    let content = fs::read_to_string(&c).unwrap();
    assert_eq!(content.trim(), "dep-part probe-part", "got: {content:?}");
}
