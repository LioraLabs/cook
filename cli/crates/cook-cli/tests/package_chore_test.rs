//! Integration test for `cook package` (M2.5 replacement of cargo xtask
//! package). Builds cook for the host triple, runs the chore, and inspects
//! the produced tarball.
//!
//! Skipped on platforms not yet supported by Phase 1 (i.e. anything other
//! than linux-{amd64,arm64} and darwin-{amd64,arm64}).

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn host_supported() -> bool {
    let out = Command::new("rustc").arg("-vV").output().expect("rustc -vV");
    let text = String::from_utf8_lossy(&out.stdout);
    let triple = text
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap_or("");
    let os_ok = triple.contains("linux") || triple.contains("apple-darwin");
    let arch_ok = triple.starts_with("x86_64-") || triple.starts_with("aarch64-");
    os_ok && arch_ok
}

#[test]
fn chore_package_produces_expected_tarball_shape() {
    if !host_supported() {
        eprintln!("skipping: host not in Phase 1 support matrix");
        return;
    }

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("cli/crates/cook-cli/.. = repo root");

    // Use a sentinel version that won't collide with real release tarballs.
    let version = "v0.0.0-test";

    let triple_out = Command::new("rustc").arg("-vV").output().expect("rustc -vV");
    let triple_text = String::from_utf8_lossy(&triple_out.stdout);
    let triple = triple_text
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("host line")
        .to_string();

    // Pre-clean stage + dist dirs so the test exercises the from-clean
    // codepath. The chore-dep wiring (chore package: build-lua bundle-luarocks)
    // means each invocation must rebuild the staged tree from scratch.
    let _ = std::fs::remove_dir_all(repo.join("target/cook-stage"));
    let _ = std::fs::remove_dir_all(repo.join("cli/target/dist"));

    // Build cook explicitly so the chore has a release binary to invoke.
    let build_status = Command::new("cargo")
        .args(["build", "--release", "-p", "cook-cli"])
        .current_dir(repo.join("cli"))
        .status()
        .expect("cargo build cook-cli");
    assert!(build_status.success(), "cargo build cook-cli failed");

    let cook_bin = repo.join("cli/target/release/cook");
    let chore_status = Command::new(&cook_bin)
        .args([
            "--set",
            &format!("VERSION={}", version),
            "--set",
            &format!("TARGET={}", triple),
            "package",
        ])
        .current_dir(repo)
        .status()
        .expect("cook package");
    assert!(chore_status.success(), "cook package failed");

    let os_part = if triple.contains("apple-darwin") {
        "darwin"
    } else {
        "linux"
    };
    let arch_part = if triple.starts_with("x86_64-") {
        "amd64"
    } else {
        "arm64"
    };
    let tarball = repo.join(format!(
        "cli/target/dist/cook-{}-{}-{}.tar.gz",
        version, os_part, arch_part
    ));
    assert!(tarball.exists(), "tarball missing at {}", tarball.display());

    let sha_sibling = repo.join(format!(
        "cli/target/dist/cook-{}-{}-{}.tar.gz.sha256",
        version, os_part, arch_part
    ));
    assert!(
        sha_sibling.exists(),
        "sha256 sibling missing at {}",
        sha_sibling.display()
    );

    // Inspect tarball entries.
    let listing = Command::new("tar")
        .args(["-tzf", tarball.to_str().unwrap()])
        .output()
        .expect("tar -tzf");
    assert!(listing.status.success());
    let entries = String::from_utf8_lossy(&listing.stdout);

    let must_contain = [
        "VERSION",
        "bin/cook",
        "bin/lua",
        "bin/luac",
        "bin/luarocks",
        "include/lua5.4/lua.h",
        "include/lua5.4/luaconf.h",
        "share/luarocks/cmd.lua",
        "share/cook/default-rocks-config.lua",
    ];
    for needle in must_contain {
        assert!(
            entries
                .lines()
                .any(|l| l.ends_with(needle) || l.contains(&format!("/{}", needle))),
            "tarball missing expected entry: {}\nfull listing:\n{}",
            needle,
            entries
        );
    }
    // Per-platform shared lib.
    let shared_lib = if os_part == "darwin" {
        "lib/liblua5.4.dylib"
    } else {
        "lib/liblua5.4.so"
    };
    assert!(
        entries.lines().any(|l| l.contains(shared_lib)),
        "tarball missing shared lib: {}",
        shared_lib
    );
}
