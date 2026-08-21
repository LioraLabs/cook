use super::*;
use cook_contracts::probe_key::LocalProbeKey;

#[test]
fn empty_seal_set_is_zero() {
    let store = ProbeValueStore::new();
    assert_eq!(seal_contribution(&BTreeSet::new(), &store), 0);
}

#[test]
fn seal_contribution_depends_on_value() {
    let store = ProbeValueStore::new();
    store.insert(&LocalProbeKey::new("host"), b"\"x86_64-linux\"\n".to_vec());
    let mut s = BTreeSet::new();
    s.insert(LocalProbeKey::new("host"));
    let a = seal_contribution(&s, &store);

    let store2 = ProbeValueStore::new();
    store2.insert(&LocalProbeKey::new("host"), b"\"aarch64-darwin\"\n".to_vec());
    let b = seal_contribution(&s, &store2);
    assert_ne!(a, b, "different sealed host value must change the contribution");
}

#[test]
fn seal_contribution_order_insensitive() {
    // BTreeSet already sorts; this guards the render is sorted by key.
    let store = ProbeValueStore::new();
    store.insert(&LocalProbeKey::new("a"), b"1\n".to_vec());
    store.insert(&LocalProbeKey::new("b"), b"2\n".to_vec());
    let mut s1 = BTreeSet::new();
    s1.insert(LocalProbeKey::new("a"));
    s1.insert(LocalProbeKey::new("b"));
    let mut s2 = BTreeSet::new();
    s2.insert(LocalProbeKey::new("b"));
    s2.insert(LocalProbeKey::new("a"));
    assert_eq!(seal_contribution(&s1, &store), seal_contribution(&s2, &store));
}

#[test]
fn distinct_keys_same_values_differ_from_swapped() {
    // Key bytes are part of the record, so {a=1,b=2} != {a=2,b=1}.
    let store = ProbeValueStore::new();
    store.insert(&LocalProbeKey::new("a"), b"1".to_vec());
    store.insert(&LocalProbeKey::new("b"), b"2".to_vec());
    let mut s = BTreeSet::new();
    s.insert(LocalProbeKey::new("a"));
    s.insert(LocalProbeKey::new("b"));
    let forward = seal_contribution(&s, &store);

    let swapped = ProbeValueStore::new();
    swapped.insert(&LocalProbeKey::new("a"), b"2".to_vec());
    swapped.insert(&LocalProbeKey::new("b"), b"1".to_vec());
    assert_ne!(forward, seal_contribution(&s, &swapped));
}
