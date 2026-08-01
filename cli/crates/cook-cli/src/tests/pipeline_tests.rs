use super::*;

// The five `no_auto_gc_env_value_enables` cases below take the candidate
// env-var value as a plain `Option<&str>` parameter, so none of them touch
// the actual process environment — no shared-state race is possible.

#[test]
fn env_value_unset_does_not_enable() {
    assert!(!no_auto_gc_env_value_enables(None));
}

#[test]
fn env_value_1_enables() {
    assert!(no_auto_gc_env_value_enables(Some("1")));
}

#[test]
fn env_value_0_does_not_enable() {
    assert!(!no_auto_gc_env_value_enables(Some("0")));
}

#[test]
fn env_value_empty_does_not_enable() {
    assert!(!no_auto_gc_env_value_enables(Some("")));
}

#[test]
fn env_value_arbitrary_nonempty_enables() {
    assert!(no_auto_gc_env_value_enables(Some("yes-please")));
}

/// Restores `COOK_NO_AUTO_GC` to its pre-test value on drop, so an assertion
/// that fires mid-test cannot leak a set variable into the rest of the test
/// binary. Restoring in a plain tail statement would be skipped on unwind.
struct EnvRestore(Option<String>);

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("COOK_NO_AUTO_GC", v),
            None => std::env::remove_var("COOK_NO_AUTO_GC"),
        }
    }
}

/// Proves `no_auto_gc_enabled` actually wires the `--no-auto-gc` flag and the
/// `COOK_NO_AUTO_GC` env var together (the pure-helper tests above only cover
/// the value semantics in isolation).
///
/// This is the one test here that mutates the process environment, and the
/// mitigation is deliberately partial — read what it does and does not buy.
/// Cargo runs a crate's unit tests as parallel threads in ONE process, so
/// `set_var` racing another thread's `getenv` is a genuine data race (it is
/// `unsafe` as of edition 2024). `#[serial_test::serial]` only mutually
/// excludes other `#[serial]` tests; it does NOT stop the crate's ordinary
/// parallel tests from calling `getenv` underneath (several reach it
/// indirectly, e.g. via `resolve_project_root` / `dirs::cache_dir`). It is
/// therefore a narrowing, not a fix.
///
/// It is acceptable here because the variable this test writes,
/// `COOK_NO_AUTO_GC`, has exactly one reader in the entire crate —
/// `no_auto_gc_enabled` — and no other test invokes it concurrently. The
/// value semantics are covered race-free by the pure `no_auto_gc_env_value_enables`
/// cases above; all this test adds is that the flag and the env var really are
/// OR-ed together, which cannot be observed without touching the environment.
/// `EnvRestore` puts the variable back even on unwind.
#[test]
#[serial_test::serial]
fn no_auto_gc_enabled_wires_flag_and_env() {
    let _restore = EnvRestore(std::env::var("COOK_NO_AUTO_GC").ok());
    std::env::remove_var("COOK_NO_AUTO_GC");

    let mut globals = Globals::default();
    assert!(
        !no_auto_gc_enabled(&globals),
        "neither the flag nor the env var is set"
    );

    globals.no_auto_gc = true;
    assert!(no_auto_gc_enabled(&globals), "flag alone enables it");
    globals.no_auto_gc = false;

    std::env::set_var("COOK_NO_AUTO_GC", "1");
    assert!(no_auto_gc_enabled(&globals), "COOK_NO_AUTO_GC=1 enables it");

    std::env::set_var("COOK_NO_AUTO_GC", "0");
    assert!(
        !no_auto_gc_enabled(&globals),
        "COOK_NO_AUTO_GC=0 must not enable it"
    );

    std::env::set_var("COOK_NO_AUTO_GC", "");
    assert!(
        !no_auto_gc_enabled(&globals),
        "COOK_NO_AUTO_GC=\"\" must not enable it"
    );

    std::env::set_var("COOK_NO_AUTO_GC", "yes-please");
    assert!(
        no_auto_gc_enabled(&globals),
        "an arbitrary non-empty COOK_NO_AUTO_GC value enables it"
    );

    // `_restore` puts the variable back on drop, including on unwind.
}

/// COOK-406: `cook cache verify --json` hand-rolled its output with an escaper
/// that handled `\` and `"` only. Engine error strings routinely carry
/// newlines, so a verify failure emitted invalid JSON on a machine surface.
#[test]
fn verify_json_survives_a_multiline_error_detail() {
    use cook_engine::verify::{UnitReport, UnitVerdict, VerifyReport};

    let report = VerifyReport {
        units: vec![
            UnitReport {
                recipe: "build".into(),
                unit: "out.o".into(),
                key: "abc123".into(),
                verdict: UnitVerdict::Error {
                    detail: "re-run failed:\n  cc: no such file\n\ttab\there".into(),
                },
            },
            UnitReport {
                recipe: "pack".into(),
                unit: r#"we"ird\path"#.into(),
                key: "def456".into(),
                verdict: UnitVerdict::Divergence {
                    detail: "bytes differ\r\nat offset 12".into(),
                },
            },
        ],
    };

    let rendered = verify_json_value(&report).to_string();
    let parsed: serde_json::Value =
        serde_json::from_str(&rendered).expect("verify --json must emit parseable JSON");

    assert_eq!(parsed["errors"], 1);
    assert_eq!(parsed["divergences"], 1);
    assert_eq!(
        parsed["units"][0]["detail"],
        "re-run failed:\n  cc: no such file\n\ttab\there",
        "the detail must round-trip byte for byte, not merely parse"
    );
    assert_eq!(parsed["units"][1]["unit"], r#"we"ird\path"#);
    assert_eq!(parsed["units"][1]["detail"], "bytes differ\r\nat offset 12");
}
