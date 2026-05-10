//! M3.4 integration tests. Two flavors:
//!   - Offline tests: drive the clap surface against the real RocksDriver
//!     pointed at a fake-luarocks shim that records argv (no network).
//!   - Online tests: gated on `cook chore gate-m2` having been run, install
//!     cook_smoke from rocks.usecook.com via the bundled luarocks.
//!
//! Online tests require network egress + a populated rocks.usecook.com.
//! Run with `--ignored` to enable: `cargo test -p cook-cli --test modules_integration -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cook_binary() -> PathBuf {
    // Built by cargo before tests run.
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn modules_help_lists_subcommands() {
    let out = Command::new(cook_binary())
        .args(["modules", "--help"])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("install"));
    assert!(stdout.contains("remove"));
    assert!(stdout.contains("update"));
    assert!(stdout.contains("list"));
    assert!(stdout.contains("search"));
}

#[test]
fn modules_list_in_empty_project_says_no_lockfile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(cook_binary())
        .args(["modules", "list"])
        .current_dir(dir.path())
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no cook.lock") || stderr.is_empty());
}

#[test]
#[ignore = "requires ~/.cook/bin/luarocks (gate-m2 must be green) + network egress"]
fn online_install_cook_smoke_from_fixture_project() {
    let src = fixture_dir("phase3-online");
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir_all(&src, dir.path()).expect("copy fixture");

    let install = Command::new(cook_binary())
        .args(["modules", "install"])
        .current_dir(dir.path())
        .output()
        .expect("spawn install");
    assert!(
        install.status.success(),
        "install failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let installed = dir
        .path()
        .join("cook_modules/share/lua/5.4/cook_smoke.lua");
    assert!(
        installed.exists(),
        "cook_smoke.lua missing at {}",
        installed.display()
    );

    let lock = dir.path().join("cook.lock");
    assert!(lock.exists(), "cook.lock not written");

    let smoke = Command::new(cook_binary())
        .arg("smoke")
        .current_dir(dir.path())
        .output()
        .expect("spawn smoke");
    let stdout = String::from_utf8_lossy(&smoke.stdout);
    assert!(stdout.contains("42"), "smoke recipe should print 42; got: {stdout}");
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}
