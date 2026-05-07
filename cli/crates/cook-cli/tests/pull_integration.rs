//! End-to-end test: serve a fixture tarball with `mockito`, run `cook pull` in
//! a tempdir cwd against it, assert files and trust state.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use cook_cli::pull;
use flate2::write::GzEncoder;
use flate2::Compression;
use tar::{Builder, Header};
use tempfile::TempDir;

// Process-global state (env vars, current_dir) is mutated; force serial.
static SERIAL: Mutex<()> = Mutex::new(());

fn make_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buf = Vec::new();
    let gz = GzEncoder::new(buf, Compression::default());
    let mut tar = Builder::new(gz);
    for (path, body) in entries {
        let mut header = Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, *body).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

#[test]
fn pull_writes_module_into_cook_modules() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    let mut server = mockito::Server::new();
    let archive = make_archive(&[
        ("registry-abc/modules/cpp/init.lua", b"-- cpp init"),
        ("registry-abc/modules/cpp/helpers.lua", b"-- helpers"),
        ("registry-abc/modules/rust/init.lua", b"-- rust"),
        ("registry-abc/README.md", b"ignored"),
    ]);
    let _m = server
        .mock("GET", "/archive/main.tar.gz")
        .with_status(200)
        .with_body(&archive)
        .create();

    let cwd = TempDir::new().unwrap();
    let cfg = TempDir::new().unwrap();

    // Direct the orchestrator at our temp config dir via XDG_CONFIG_HOME (Linux).
    // On macOS/Windows, dirs::config_dir() does not consult XDG_CONFIG_HOME. To
    // keep the test cross-platform we instead override HOME; on Linux dirs falls
    // back to $HOME/.config when XDG_CONFIG_HOME is unset.
    let prior_xdg = env::var_os("XDG_CONFIG_HOME");
    let prior_home = env::var_os("HOME");
    let prior_cwd = env::current_dir().unwrap();

    env::set_var("XDG_CONFIG_HOME", cfg.path());
    env::set_var("HOME", cfg.path());
    env::set_current_dir(cwd.path()).unwrap();

    let argv: Vec<String> = ["pull", "cpp", "--accept-trust", "--registry", &server.url()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let code = pull::run_from_argv(&argv);

    // Restore env before assertions so a failure doesn't leak state.
    env::set_current_dir(&prior_cwd).unwrap();
    match prior_xdg {
        Some(v) => env::set_var("XDG_CONFIG_HOME", v),
        None => env::remove_var("XDG_CONFIG_HOME"),
    }
    match prior_home {
        Some(v) => env::set_var("HOME", v),
        None => env::remove_var("HOME"),
    }

    assert_eq!(code, 0);

    let init = cwd.path().join("cook_modules/cpp/init.lua");
    let helpers = cwd.path().join("cook_modules/cpp/helpers.lua");
    assert!(init.exists());
    assert!(helpers.exists());
    assert_eq!(fs::read(&init).unwrap(), b"-- cpp init");

    // Rust module was NOT requested.
    assert!(!cwd.path().join("cook_modules/rust").exists());

    // Trust file recorded the URL.
    let trust_path: PathBuf = if cfg!(target_os = "linux") {
        cfg.path().join("cook/trust.toml")
    } else if cfg!(target_os = "macos") {
        cfg.path().join("Library/Application Support/cook/trust.toml")
    } else {
        cfg.path().join("cook/trust.toml")
    };
    assert!(
        trust_path.exists(),
        "trust file missing at {}",
        trust_path.display()
    );
    let body = fs::read_to_string(&trust_path).unwrap();
    assert!(body.contains(&server.url()));
}

#[test]
fn pull_list_prints_module_names() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    let mut server = mockito::Server::new();
    let archive = make_archive(&[
        ("r/modules/cpp/init.lua", b"x"),
        ("r/modules/rust/init.lua", b"x"),
        ("r/modules/pnpm-monorepo/init.lua", b"x"),
    ]);
    let _m = server
        .mock("GET", "/archive/main.tar.gz")
        .with_status(200)
        .with_body(&archive)
        .create();

    let cfg = TempDir::new().unwrap();
    let prior_xdg = env::var_os("XDG_CONFIG_HOME");
    let prior_home = env::var_os("HOME");
    env::set_var("XDG_CONFIG_HOME", cfg.path());
    env::set_var("HOME", cfg.path());

    let argv: Vec<String> = ["pull", "--list", "--accept-trust", "--registry", &server.url()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let code = pull::run_from_argv(&argv);

    match prior_xdg {
        Some(v) => env::set_var("XDG_CONFIG_HOME", v),
        None => env::remove_var("XDG_CONFIG_HOME"),
    }
    match prior_home {
        Some(v) => env::set_var("HOME", v),
        None => env::remove_var("HOME"),
    }

    assert_eq!(code, 0);
}

#[test]
fn pull_unknown_module_errors_3() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    let mut server = mockito::Server::new();
    let archive = make_archive(&[("r/modules/cpp/init.lua", b"x")]);
    let _m = server
        .mock("GET", "/archive/main.tar.gz")
        .with_status(200)
        .with_body(&archive)
        .create();

    let cwd = TempDir::new().unwrap();
    let cfg = TempDir::new().unwrap();
    let prior_cwd = env::current_dir().unwrap();
    let prior_xdg = env::var_os("XDG_CONFIG_HOME");
    let prior_home = env::var_os("HOME");
    env::set_var("XDG_CONFIG_HOME", cfg.path());
    env::set_var("HOME", cfg.path());
    env::set_current_dir(cwd.path()).unwrap();

    let argv: Vec<String> = ["pull", "rust", "--accept-trust", "--registry", &server.url()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let code = pull::run_from_argv(&argv);

    env::set_current_dir(&prior_cwd).unwrap();
    match prior_xdg {
        Some(v) => env::set_var("XDG_CONFIG_HOME", v),
        None => env::remove_var("XDG_CONFIG_HOME"),
    }
    match prior_home {
        Some(v) => env::set_var("HOME", v),
        None => env::remove_var("HOME"),
    }

    assert_eq!(code, 3);
    // No cook_modules dir was created (we erred before the install loop).
    assert!(!cwd.path().join("cook_modules").exists());
}
