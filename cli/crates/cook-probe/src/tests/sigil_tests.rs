use super::*;
use crate::store::ProbeValueStore;

// ── COOK-361: agreement guard against re-forking the substitution paths ──
//
// Every probe-substitution position shares `cook_contracts::sigil::subst`
// (worker commands CS-0192, chore drain spawns CS-0193, register output
// patterns CS-0195), but each entry point wraps its own store lookup and
// read-view construction. This pins the store-backed wrapper to the law
// directly: the same ident over the same canonical value must render
// identically through `resolve_probe_sigils` and through `substitute`. If a
// second renderer is ever deliberately introduced, COOK-361's standing rule
// requires a new agreement test beside this one.

#[test]
fn store_backed_substitution_agrees_with_the_law() {
    use cook_contracts::probe_value::encode_canonical_json;
    use cook_contracts::sigil::{probe_ref, subst::substitute};

    let value = serde_json::json!({
        "name": "zlib",
        "cflags": ["-O2", "-Wall"],
        "version": 3.0,
    });
    let store = ProbeValueStore::new();
    store.insert("cc:zlib", encode_canonical_json(&value));

    for ident in ["cc:zlib.name", "cc:zlib.cflags[2]", "cc:zlib.version"] {
        let via_store =
            resolve_probe_sigils(&store, &format!("echo $<{ident}>")).expect("store path renders");
        let r = probe_ref(ident).expect("probe-shaped");
        let via_law = substitute(&value, r.path(), ident).expect("law renders");
        assert_eq!(via_store, format!("echo {via_law}"), "ident {ident}");
    }

    // The read view too: a tool-path annotation merged by the store side
    // must render exactly what the law renders over the merged value.
    let tools = serde_json::json!({"gcc": {"hash": "ab12"}});
    store.insert("cc:tc", encode_canonical_json(&tools));
    store.set_tool_paths(
        "cc:tc",
        std::collections::BTreeMap::from([("gcc".to_string(), "/usr/bin/gcc".to_string())]),
    );
    let via_store = resolve_probe_sigils(&store, "$<cc:tc.gcc.path>").expect("read view renders");
    let mut merged = tools.clone();
    cook_contracts::probe_value::merge_tool_paths(
        &mut merged,
        &std::collections::BTreeMap::from([("gcc".to_string(), "/usr/bin/gcc".to_string())]),
    );
    let r = probe_ref("cc:tc.gcc.path").expect("probe-shaped");
    let via_law = substitute(&merged, r.path(), "cc:tc.gcc.path").expect("law renders");
    assert_eq!(via_store, via_law);
}

/// A command with no probe reference comes back byte-identical, and a
/// `$<...>` span that is not probe-shaped is left literal — both the
/// behaviours the pre-CS-0188 register-phase rewrite had.
#[test]
fn a_command_without_a_probe_reference_is_untouched() {
    let store = ProbeValueStore::new();
    assert_eq!(
        resolve_probe_sigils(&store, "cc -c a.c -o a.o").unwrap(),
        "cc -c a.c -o a.o"
    );
    assert_eq!(
        resolve_probe_sigils(&store, "echo $<out>").unwrap(),
        "echo $<out>"
    );
}

/// CS-0152: a reference to a key nobody materialised is a hard error naming
/// the key, not a silent empty string.
#[test]
fn an_unmaterialised_key_is_the_cs0152_diagnostic() {
    let store = ProbeValueStore::new();
    let error = resolve_probe_sigils(&store, "echo $<cc:absent.name>")
        .expect_err("an unmaterialised key must fail");
    assert!(
        error.contains("cc:absent"),
        "must name the key; got: {error}"
    );
    assert!(
        error.contains("not materialised"),
        "must be the CS-0152 sentence; got: {error}"
    );
}
