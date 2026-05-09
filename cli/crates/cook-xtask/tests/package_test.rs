//! Integration tests for `cargo xtask package`.
//!
//! These tests exercise the full package::run() path: they create a tiny
//! stand-in binary, invoke the packager, decompress and inspect the tarball,
//! and verify VERSION content + bin/cook executability.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use cook_xtask::package::{run, PackageArgs};
use tempfile::TempDir;

/// Returns the host rustc target triple (e.g. "x86_64-unknown-linux-gnu").
fn host_target() -> String {
    // `rustc -vV` output includes "host: <triple>"; we extract that line.
    let out = std::process::Command::new("rustc")
        .args(["-vV"])
        .output()
        .expect("rustc must be on PATH");
    let text = String::from_utf8(out.stdout).expect("rustc output is UTF-8");
    for line in text.lines() {
        if let Some(triple) = line.strip_prefix("host: ") {
            return triple.to_string();
        }
    }
    panic!("could not determine host target from `rustc -vV`");
}

/// Creates a minimal executable shell script that prints a version line.
fn create_dummy_binary(dir: &TempDir) -> std::path::PathBuf {
    let bin_path = dir.path().join("cook");
    fs::write(&bin_path, "#!/bin/sh\necho 'cook dummy'\n").unwrap();
    let mut perms = fs::metadata(&bin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin_path, perms).unwrap();
    bin_path
}

#[test]
fn package_produces_correct_tarball_layout() {
    let binary_dir = TempDir::new().unwrap();
    let dist_dir = TempDir::new().unwrap();

    let dummy_bin = create_dummy_binary(&binary_dir);
    let version = "v0.0.1-dev";
    let target = host_target();

    let args = PackageArgs {
        binary: dummy_bin.clone(),
        version: version.to_string(),
        target: target.clone(),
    };

    let (out_path, sha256_hex) = run(&args, dist_dir.path()).unwrap();

    // Tarball must exist and have non-zero size
    assert!(
        out_path.exists(),
        "tarball not created: {}",
        out_path.display()
    );
    assert!(
        fs::metadata(&out_path).unwrap().len() > 0,
        "tarball is empty"
    );

    // sha256 must be a 64-char hex string
    assert_eq!(
        sha256_hex.len(),
        64,
        "SHA-256 hex digest should be 64 chars"
    );
    assert!(
        sha256_hex.chars().all(|c| c.is_ascii_hexdigit()),
        "SHA-256 digest is not hex: {sha256_hex}"
    );

    // Extract the tarball and inspect contents
    let extract_dir = TempDir::new().unwrap();
    let tarball_data = fs::read(&out_path).unwrap();
    let gz = flate2::read::GzDecoder::new(tarball_data.as_slice());
    let mut ar = tar::Archive::new(gz);
    ar.unpack(extract_dir.path()).unwrap();

    // Collect extracted paths (relative)
    let mut paths: Vec<String> = vec![];
    for entry in fs::read_dir(extract_dir.path()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type().unwrap().is_dir() {
            for sub in fs::read_dir(entry.path()).unwrap() {
                let sub = sub.unwrap();
                paths.push(format!("{}/{}", name, sub.file_name().to_string_lossy()));
            }
        } else {
            paths.push(name);
        }
    }
    paths.sort();

    assert!(
        paths.contains(&"VERSION".to_string()),
        "tarball missing ./VERSION; got: {paths:?}"
    );
    assert!(
        paths.contains(&"bin/cook".to_string()),
        "tarball missing ./bin/cook; got: {paths:?}"
    );
    // Phase 1 contract: EXACTLY these two entries, nothing else
    assert_eq!(
        paths,
        vec!["VERSION", "bin/cook"],
        "tarball has unexpected entries: {paths:?}"
    );

    // VERSION content must be "v0.0.1-dev\n"
    let version_content = fs::read_to_string(extract_dir.path().join("VERSION")).unwrap();
    assert_eq!(
        version_content,
        format!("{version}\n"),
        "VERSION file content mismatch"
    );

    // bin/cook must be executable
    let bin_path = extract_dir.path().join("bin").join("cook");
    let perms = fs::metadata(&bin_path).unwrap().permissions();
    assert!(
        perms.mode() & 0o111 != 0,
        "bin/cook is not executable; mode = {:o}",
        perms.mode()
    );
}

#[test]
fn package_checksum_file_is_written() {
    let binary_dir = TempDir::new().unwrap();
    let dist_dir = TempDir::new().unwrap();

    let dummy_bin = create_dummy_binary(&binary_dir);
    let args = PackageArgs {
        binary: dummy_bin,
        version: "v0.0.2-test".to_string(),
        target: host_target(),
    };

    let (_, sha256_hex) = run(&args, dist_dir.path()).unwrap();

    let checksums_path = dist_dir
        .path()
        .join("target")
        .join("dist")
        .join("cook-v0.0.2-test-checksums.txt");
    assert!(
        checksums_path.exists(),
        "checksums file not created: {}",
        checksums_path.display()
    );
    let content = fs::read_to_string(&checksums_path).unwrap();
    assert!(
        content.contains(&sha256_hex),
        "checksums file missing expected hash; content:\n{content}"
    );
}
