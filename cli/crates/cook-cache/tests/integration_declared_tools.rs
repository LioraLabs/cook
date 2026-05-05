//! CS-0052: Declared toolchain pinning end-to-end through cloud_key.
//!
//! Spec §6.2 acceptance:
//! 1. Empty `[cache] tools` (or absent) produces a deterministic
//!    `declared_tools_hash` of `0` — sentinel value that folds neutrally.
//! 2. Adding a tool to the list changes every step's `cloud_key` (one-shot
//!    full rebuild on toggle, per spec §10 open question 1).
//! 3. A misdeclared tool errors at probe time, before any step is registered.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use cook_cache::backend::{cloud_key, CloudKeyInputs};
use cook_cache::context::{
    compute_declared_tools_hash, DeclaredToolError, ExecutionContext,
};
use cook_cache::store::CACHE_VERSION;

/// Process-wide lock for tests that mutate `PATH`. Mirrors the lock the
/// cook-fingerprint unit tests use; a separate static here is fine because
/// these are different binaries (cargo runs each integration test as its
/// own process), and within this binary the lock is the single source of
/// truth.
static PATH_TEST_LOCK: Mutex<()> = Mutex::new(());

fn make_fake_tool(dir: &Path, name: &str, contents: &[u8]) {
    let p = dir.join(name);
    std::fs::write(&p, contents).expect("write fake tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&p).expect("meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).expect("chmod");
    }
}

fn with_path_prefix<F, R>(extra: &Path, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = PATH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let original = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", extra.display(), original);
    // SAFETY: env mutation under PATH_TEST_LOCK; no other code path in this
    // test binary mutates PATH outside the lock.
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    let r = f();
    unsafe {
        std::env::set_var("PATH", &original);
    }
    r
}

#[test]
fn empty_declared_tools_yields_sentinel_zero() {
    let cache = Mutex::new(HashMap::new());
    let (h, map) = compute_declared_tools_hash(&[], &cache).expect("ok");
    assert_eq!(h, 0, "empty input MUST hash to sentinel zero (spec §3.2.1)");
    assert!(map.is_empty());
}

#[test]
fn declared_tool_changes_cloud_key() {
    // Two ExecutionContexts on the same machine, identical except for the
    // declared-tool list. The same logical step (same command, same env,
    // same inputs) must produce different cloud_keys — proof that
    // declared_tools_hash threads through to the SHA-256 cloud_key.
    let dir = tempfile::tempdir().expect("tempdir");
    make_fake_tool(dir.path(), "cs0035tool", b"v1");

    with_path_prefix(dir.path(), || {
        let ctx_empty = ExecutionContext::probe_with_declared_tools(&[]).expect("empty ok");
        let ctx_decl = ExecutionContext::probe_with_declared_tools(&[
            "cs0035tool".to_string(),
        ])
        .expect("declared ok");

        let cmd = "/bin/sh -c true";
        let ch_empty = ctx_empty.step_context_hash(cmd);
        let ch_decl = ctx_decl.step_context_hash(cmd);
        assert_ne!(
            ch_empty, ch_decl,
            "step_context_hash MUST change when declared tools are added"
        );

        let inputs_for = |context_hash: u64| CloudKeyInputs {
            schema_version: CACHE_VERSION,
            recipe_namespace: "proj/Cookfile::build",
            command_hash: 0x1111,
            context_hash,
            env_contribution: 0,
            sorted_input_content_hashes: &[],
        };
        let key_empty = cloud_key(&inputs_for(ch_empty));
        let key_decl = cloud_key(&inputs_for(ch_decl));
        assert_ne!(
            key_empty, key_decl,
            "cloud_key MUST differ when declared tools change — \
             this is the cache-invalidation contract"
        );
    });
}

#[test]
fn missing_declared_tool_errors_at_probe() {
    // The whole point of explicit declaration is loud failure on misdeclaration
    // (spec §3.2.2). A tool not on PATH must error at probe — not silently
    // produce a degraded hash.
    let names = vec!["cs0035-definitely-not-installed-zzz".to_string()];
    let result = ExecutionContext::probe_with_declared_tools(&names);
    let err = match result {
        Ok(_) => panic!("missing tool MUST error, got Ok"),
        Err(e) => e,
    };
    let msg = err.to_string();
    match err {
        DeclaredToolError::NotFound { name, .. } => {
            assert_eq!(name, "cs0035-definitely-not-installed-zzz");
        }
        DeclaredToolError::Canonicalize { .. } => {
            panic!("expected NotFound, got Canonicalize");
        }
    }
    // Diagnostic must mention devcontainer/nix-shell so users see the
    // intended deployment model in the error itself.
    assert!(
        msg.contains("devcontainer") && msg.contains("nix-shell"),
        "diagnostic SHOULD mention devcontainer/nix-shell as remediation: {msg}"
    );
}

#[test]
fn declared_tool_content_change_changes_cloud_key() {
    // Mirrors the cross-machine scenario the spec exists to close: same tool
    // *name* on two machines, different *bytes*. cloud_key MUST diverge so
    // the cache won't serve machine-A's bytes to machine-B.
    let dir_a = tempfile::tempdir().expect("tempdir A");
    make_fake_tool(dir_a.path(), "cs0035drift", b"contents A");

    let dir_b = tempfile::tempdir().expect("tempdir B");
    make_fake_tool(dir_b.path(), "cs0035drift", b"contents B");

    let names = vec!["cs0035drift".to_string()];

    let cmd = "/bin/sh -c true";
    let inputs_for = |context_hash: u64| CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/Cookfile::build",
        command_hash: 0x1111,
        context_hash,
        env_contribution: 0,
        sorted_input_content_hashes: &[],
    };

    let key_a = with_path_prefix(dir_a.path(), || {
        let ctx = ExecutionContext::probe_with_declared_tools(&names).expect("a ok");
        cloud_key(&inputs_for(ctx.step_context_hash(cmd)))
    });
    let key_b = with_path_prefix(dir_b.path(), || {
        let ctx = ExecutionContext::probe_with_declared_tools(&names).expect("b ok");
        cloud_key(&inputs_for(ctx.step_context_hash(cmd)))
    });

    assert_ne!(
        key_a, key_b,
        "different binary content for the same declared name MUST yield different cloud_keys"
    );
}
