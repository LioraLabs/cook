use super::*;
use cook_contracts::sigil::probe_ref;

#[test]
fn parses_probe_ref_bare_key() {
    let r = probe_ref("cc:zlib").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(r.path(), &[]);
    assert_eq!(lua_access(&r), r#"cook.probes.get("cc:zlib")"#);
}

#[test]
fn parses_probe_ref_field() {
    let r = probe_ref("cc:zlib.cflags").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(r.path(), &[Seg::Field("cflags".to_string())]);
    assert_eq!(lua_access(&r), r#"cook.probes.get("cc:zlib").cflags"#);
}

#[test]
fn parses_probe_ref_field_indexed() {
    let r = probe_ref("cc:zlib.cflags[2]").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(
        r.path(),
        &[Seg::Field("cflags".to_string()), Seg::Index("2".to_string())]
    );
    assert_eq!(lua_access(&r), r#"cook.probes.get("cc:zlib").cflags[2]"#);
}

#[test]
fn probe_ref_index_directly_on_the_key() {
    let r = probe_ref("list:items[1]").expect("probe-shaped");
    assert_eq!(r.key(), "list:items");
    assert_eq!(lua_access(&r), r#"cook.probes.get("list:items")[1]"#);
}

#[test]
fn probe_ref_escapes_the_key_for_the_lua_literal() {
    // Unreachable through `scan` (the IDENT charset admits neither `"` nor
    // `\`), but `probe_ref` is public: one escape rule, not three. Before
    // COOK-357 the resolver escaped `\` and `"`, unit_api additionally
    // escaped `\n`/`\r`/`\0`, and neither agreed with the other.
    let r = probe_ref(r#"k:a"b"#).expect("probe-shaped");
    assert_eq!(lua_access(&r), r#"cook.probes.get("k:a\"b")"#);
}
