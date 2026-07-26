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
/// `#[serial_test::serial]` is load-bearing, not decoration: cargo runs a
/// crate's unit tests as parallel threads in ONE process, so `set_var` racing
/// another thread's `getenv` is a genuine data race (it is `unsafe` as of
/// edition 2024). Nothing else reads `COOK_NO_AUTO_GC` today, but the crate
/// already depends on `serial_test` for exactly this pattern (see
/// `modules/tests/driver_tests.rs`), so there is no reason to rely on that.
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
