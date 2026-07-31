//! CS-0195 deleted `lua_access`, the last Lua render of a probe ref; the
//! parse (and its tests) live in `cook_contracts::sigil`. This module keeps
//! one smoke test that the re-export stays wired.

#[test]
fn reexport_stays_wired() {
    let r = super::probe_ref("cc:zlib.cflags[2]").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(r.path().len(), 2);
}
