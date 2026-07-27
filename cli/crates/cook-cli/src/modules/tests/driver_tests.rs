use super::*;
use std::path::{Path, PathBuf};

fn fake_prefix() -> tempfile::TempDir {
    // Set up a fake $prefix where bin/luarocks is a symlink to the
    // tests/fixtures/driver/fake-luarocks.sh script.
    let tmp = tempfile::tempdir().expect("tempdir");
        let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/driver/fake-luarocks.sh");
    std::os::unix::fs::symlink(&fake, bin.join("luarocks")).expect("symlink");
    tmp
}

fn read_argv_log(path: &Path) -> Vec<String> {
    let raw = std::fs::read_to_string(path).expect("read log");
    raw.lines()
        .skip(1) // skip "argv:" header
        .map(|l| l.trim().to_string())
        .collect()
}

fn clear_fake_env() {
    for var in ["FAKE_LUAROCKS_LOG", "FAKE_LUAROCKS_EXIT", "FAKE_LUAROCKS_STDOUT", "FAKE_LUAROCKS_STDERR"] {
        std::env::remove_var(var);
    }
}

#[test]
#[serial_test::serial]
fn install_argv_includes_tree_and_servers() {
    clear_fake_env();
    let prefix = fake_prefix();
    let project = tempfile::tempdir().expect("project");
    let log = project.path().join("argv.log");
    std::env::set_var("FAKE_LUAROCKS_LOG", &log);
    std::env::set_var("FAKE_LUAROCKS_EXIT", "0");

    let driver = RocksDriver::new(
        prefix.path().to_path_buf(),
        vec![
            "https://rocks.usecook.com".to_string(),
            "https://luarocks.org".to_string(),
        ],
        project.path().to_path_buf(),
    );
    driver.install("cook_smoke", "*").expect("install");

    let argv = read_argv_log(&log);
    assert_eq!(argv[0], "install");
    assert!(argv.iter().any(|a| a == "--tree"));
    let tree_idx = argv.iter().position(|a| a == "--tree").unwrap();
    assert!(argv[tree_idx + 1].ends_with("cook_modules"));
    let server_args: Vec<&String> = argv
        .iter()
        .filter(|a| a.starts_with("--server="))
        .collect();
    // Exactly ONE --server flag: luarocks' flag is single-valued
    // (last-wins), and luarocks.org is already in its built-in default
    // server list, so the blessed index is the only flag emitted.
    assert_eq!(
        server_args,
        vec![&"--server=https://rocks.usecook.com".to_string()]
    );
    assert_eq!(argv.last().unwrap(), "cook_smoke");
    // Constraint "*" omitted from argv (passes through as no-constraint).
    assert!(!argv.iter().any(|a| a == "*"));
    clear_fake_env();
}

#[test]
#[serial_test::serial]
fn install_with_explicit_constraint_passes_through() {
    clear_fake_env();
    let prefix = fake_prefix();
    let project = tempfile::tempdir().expect("project");
    let log = project.path().join("argv.log");
    std::env::set_var("FAKE_LUAROCKS_LOG", &log);
    std::env::set_var("FAKE_LUAROCKS_EXIT", "0");

    let driver = RocksDriver::new(
        prefix.path().to_path_buf(),
        Vec::new(),
        project.path().to_path_buf(),
    );
    driver.install("argparse", ">=0.7").expect("install");
    let argv = read_argv_log(&log);
    assert_eq!(argv[0], "install");
    assert!(argv.iter().any(|a| a == "argparse"));
    assert!(argv.iter().any(|a| a == ">=0.7"));
    clear_fake_env();
}

#[test]
#[serial_test::serial]
fn install_locked_uses_pinned_name_and_version() {
    clear_fake_env();
    let prefix = fake_prefix();
    let project = tempfile::tempdir().expect("project");
    let log = project.path().join("argv.log");
    std::env::set_var("FAKE_LUAROCKS_LOG", &log);

    let driver = RocksDriver::new(
        prefix.path().to_path_buf(),
        vec!["https://example".into()],
        project.path().to_path_buf(),
    );
    let locked = LockedModule {
        name: "cook_smoke".into(),
        version: "0.1.0-1".into(),
        source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
        integrity: "sha256-x".into(),
        direct: true,
    };
    driver.install_locked(&locked).expect("install_locked");
    let argv = read_argv_log(&log);
    // name + exact version, never the source URL (luarocks cannot
    // resolve a git/tarball URL passed as a package spec).
    assert_eq!(argv[argv.len() - 2], locked.name);
    assert_eq!(argv.last().unwrap(), &locked.version);
    clear_fake_env();
}

#[test]
#[serial_test::serial]
fn nonzero_exit_passes_through_argv_stdout_stderr() {
    clear_fake_env();
    let prefix = fake_prefix();
    let project = tempfile::tempdir().expect("project");
    let log = project.path().join("argv.log");
    std::env::set_var("FAKE_LUAROCKS_LOG", &log);
    std::env::set_var("FAKE_LUAROCKS_EXIT", "7");
    std::env::set_var("FAKE_LUAROCKS_STDOUT", "stdout-marker");
        std::env::set_var("FAKE_LUAROCKS_STDERR", "stderr-marker");

    let driver = RocksDriver::new(
        prefix.path().to_path_buf(),
        Vec::new(),
        project.path().to_path_buf(),
    );
    let err = driver.remove("cook_smoke").expect_err("must fail");
    let msg = format!("{:#}", err);
    assert!(msg.contains("luarocks failed"));
    assert!(msg.contains("stdout-marker"));
        assert!(msg.contains("stderr-marker"));
    assert!(msg.contains("exit 7"));
    clear_fake_env();
}

#[test]
fn parse_list_output_extracts_porcelain() {
    let stdout = b"cook_smoke\t0.1.0-1\tinstalled\nargparse\t0.7.1-1\tinstalled\n";
    let rocks = parse_list_output(stdout);
    assert_eq!(rocks.len(), 2);
    assert_eq!(rocks[0].name, "cook_smoke");
    assert_eq!(rocks[0].version, "0.1.0-1");
    assert_eq!(rocks[1].name, "argparse");
    assert_eq!(rocks[1].version, "0.7.1-1");
}

#[test]
fn parse_search_output_extracts_name_version() {
    let stdout = b"cook_smoke (0.1.0-1)\nlua-cjson (2.1.0.10-1)\n";
    let hits = parse_search_output(stdout, &["https://rocks.usecook.com".into()]);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].name, "cook_smoke");
    assert_eq!(hits[0].version, "0.1.0-1");
    assert_eq!(hits[0].index, "https://rocks.usecook.com");
}

#[test]
fn truncate_stream_caps_long_output() {
    let mut big = vec![b'A'; 64 * 1024 + 100];
    big.extend_from_slice(b"\ntail diagnostic");
    let truncated = cook_contracts::CapturedStream::from_bytes(&big);
    let truncated = truncated.as_str();
    assert!(truncated.contains("bytes elided"));
    assert!(truncated.contains("tail diagnostic"));
    assert!(truncated.len() < big.len());
}

#[test]
fn truncate_stream_is_utf8_safe_across_the_old_byte_boundary() {
    let mut big = vec![b'A'; 64 * 1024 - 1];
    big.extend_from_slice("é".as_bytes());
    big.extend(std::iter::repeat_n(b'Z', 100));
    let truncated = cook_contracts::CapturedStream::from_bytes(&big);
    assert!(truncated.as_str().contains("ZZZZ"));
}
