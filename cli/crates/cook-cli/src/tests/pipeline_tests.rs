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

/// Proves `no_auto_gc_enabled` actually wires the `--no-auto-gc` flag and the
/// `COOK_NO_AUTO_GC` env var together (the pure-helper tests above only cover
/// the value semantics in isolation). This is the only test in the crate that
/// mutates `COOK_NO_AUTO_GC`; every env manipulation is saved and restored
/// within this single test function so its steps run sequentially and no
/// other test can observe an intermediate value.
#[test]
fn no_auto_gc_enabled_wires_flag_and_env() {
    let prev = std::env::var("COOK_NO_AUTO_GC").ok();
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

    match prev {
        Some(v) => std::env::set_var("COOK_NO_AUTO_GC", v),
        None => std::env::remove_var("COOK_NO_AUTO_GC"),
    }
}
