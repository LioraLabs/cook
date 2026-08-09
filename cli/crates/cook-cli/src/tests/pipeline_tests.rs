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

// ---------------------------------------------------------------------------
// The mirrored node/recipe vocabularies (COOK-421)
// ---------------------------------------------------------------------------
//
// `cook_engine::NodeKind` and `cook_progress::NodeKind` are two declarations
// of one vocabulary, joined by `translate_kind`; `RecipeKind` is the same
// shape a third time. This crate is the only place all of them are visible,
// so per the deliberate-copy protocol the agreement test lives here.
//
// Nothing asserted that the translation was the identity, and nothing
// asserted the wire spelling `cook-logs` reads back out of `.cook/logs`.

/// Every engine node kind reaches the same-named renderer kind.
///
/// The `match` is exhaustive on purpose: a variant added to one side and not
/// the other stops this test compiling, which is the check the two mirrors
/// never had.
#[test]
fn every_node_kind_translates_to_the_same_name() {
    use cook_engine::NodeKind as E;
    use cook_progress::NodeKind as P;
    for engine in [E::Compile, E::Link, E::Resolve, E::Generate, E::Write, E::Test, E::Cooked] {
        let expected = match engine {
            E::Compile => P::Compile,
            E::Link => P::Link,
            E::Resolve => P::Resolve,
            E::Generate => P::Generate,
            E::Write => P::Write,
            E::Test => P::Test,
            E::Cooked => P::Cooked,
        };
        assert_eq!(translate_kind(engine), expected, "{engine:?} translated to the wrong kind");
    }
}

/// The kind a node carries when nobody annotated it must be the same default
/// on both sides, or an unannotated node changes verb across the bridge.
#[test]
fn the_unannotated_default_agrees_on_both_sides() {
    assert_eq!(
        translate_kind(cook_engine::NodeKind::default()),
        cook_progress::NodeKind::default()
    );
    assert_eq!(cook_progress::NodeKind::default(), cook_progress::NodeKind::Cooked);
}

/// The renderer's kind is serialised into `.cook/logs` and read back by
/// `cook-logs`, so its spelling is a wire format, not a rendering detail.
#[test]
fn the_node_kind_wire_spelling_is_kebab_case() {
    use cook_progress::NodeKind as P;
    for (kind, spelled) in [
        (P::Compile, "compile"),
        (P::Link, "link"),
        (P::Resolve, "resolve"),
        (P::Generate, "generate"),
        (P::Write, "write"),
        (P::Test, "test"),
        (P::Cooked, "cooked"),
    ] {
        assert_eq!(serde_json::to_string(&kind).expect("serialise"), format!("\"{spelled}\""));
    }
}

/// `RecipeKind` is the same vocabulary declared a THIRD time: the author's
/// declaration (`cook_contracts::registration::RecipeKind`), the engine's
/// mirror (mapped from it in `cook-engine/src/run.rs`), and the renderer's
/// (mapped from that here). Two translations, one fact.
///
/// This asserts the wire spelling and the variant NAMES, not the mapping.
/// Re-deriving the mapping the way the code does would pass by construction —
/// the transposed arm that made the node-kind test above go red left such a
/// version of this one green.
#[test]
fn the_recipe_kind_vocabulary_is_one_vocabulary() {
    use cook_progress::event::RecipeKind as Rendered;
    for (rendered, spelled) in [(Rendered::Recipe, "recipe"), (Rendered::Chore, "chore")] {
        assert_eq!(serde_json::to_string(&rendered).expect("serialise"), format!("\"{spelled}\""));
    }
    // The three declarations name their variants identically, which is the
    // whole reason two hand-written translations have been able to look
    // right. Compared as text so nothing here re-implements the mapping.
    let names = |debug: String| debug;
    assert_eq!(names(format!("{:?}", cook_contracts::registration::RecipeKind::Chore)), "Chore");
    assert_eq!(names(format!("{:?}", cook_engine::RecipeKind::Chore)), "Chore");
    assert_eq!(names(format!("{:?}", Rendered::Chore)), "Chore");
}
