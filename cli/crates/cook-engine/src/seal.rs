//! COOK-161: execute-phase fold of a unit's effective seal set into a single
//! `seal_contribution` determinant.
//!
//! The seal KEY set is register-time data (carried on `CacheMeta.seal_keys`);
//! the VALUE fold is execute-time, because a probe's value only materialises
//! during the DAG walk once the probe has run. This mirrors how
//! `env_contribution` is the value fold of the register-time consulted_env_keys.
//! A sealing unit depends on its sealed probes (the register surface unions the
//! seal keys into the unit's probe-dependency set), so by the time a unit's
//! cache is checked or its outputs are committed, every sealed probe's value is
//! present in the `ProbeValueStore`.

use std::collections::{BTreeMap, BTreeSet};

use cook_luaotp::ProbeValueStore;

/// Resolve the effective seal set to its canonical `key -> value` map, the form
/// persisted on a `DeterminantManifest.sealed_probes` and recomputed on the
/// consumer side by `cook why`.
///
/// C2 (COOK-91 review): this is the SINGLE source of the absent-probe encoding
/// rule. A sealed key absent from the store folds to an **empty string** —
/// mirroring `seal_contribution`'s empty-bytes fold and the bytes a verifier
/// recomposing the digest from `sealed_probes` would see. Producer and consumer
/// MUST agree, or a shared-miss diff in `cook why` falsely reports a probe
/// difference. The probe-dependency wiring makes the absent case unreachable in
/// practice; the empty-string fold is the safe, digest-consistent fallback.
///
/// Values are decoded as UTF-8 (lossy guards the theoretically-impossible
/// non-UTF-8 case — probe values are canonical JSON).
pub(crate) fn resolve_sealed_probes(
    seal: &BTreeSet<String>,
    store: &ProbeValueStore,
) -> BTreeMap<String, String> {
    seal.iter()
        .map(|k| {
            let value = store
                .get(k)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            (k.clone(), value)
        })
        .collect()
}

/// xxh3_64 of the unit's *effective seal set* rendered as sorted
/// `key\0<canonical-json-bytes>` records joined by `\n`. Returns 0 for an empty
/// set so unsealed units carry no seal contribution (their key is unchanged by
/// this determinant apart from the `CACHE_VERSION` bump).
///
/// The `seal` set is a `BTreeSet`, so iteration is already sorted by key — the
/// rendering is order-insensitive in the author's declaration order. A sealed
/// key absent from the store (its probe produced no value) folds in as an empty
/// value: the determinant is still distinguished by its key, and the unit's
/// probe-dependency wiring guarantees the value is present in practice.
pub(crate) fn seal_contribution(seal: &BTreeSet<String>, store: &ProbeValueStore) -> u64 {
    if seal.is_empty() {
        return 0;
    }
    let mut buf: Vec<u8> = Vec::new();
    for (i, key) in seal.iter().enumerate() {
        if i > 0 {
            buf.push(b'\n');
        }
        buf.extend_from_slice(key.as_bytes());
        buf.push(0u8);
        if let Some(bytes) = store.get(key) {
            buf.extend_from_slice(&bytes);
        }
    }
    xxhash_rust::xxh3::xxh3_64(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_seal_set_is_zero() {
        let store = ProbeValueStore::new();
        assert_eq!(seal_contribution(&BTreeSet::new(), &store), 0);
    }

    #[test]
    fn seal_contribution_depends_on_value() {
        let store = ProbeValueStore::new();
        store.insert("host", b"\"x86_64-linux\"\n".to_vec());
        let mut s = BTreeSet::new();
        s.insert("host".to_string());
        let a = seal_contribution(&s, &store);

        let store2 = ProbeValueStore::new();
        store2.insert("host", b"\"aarch64-darwin\"\n".to_vec());
        let b = seal_contribution(&s, &store2);
        assert_ne!(a, b, "different sealed host value must change the contribution");
    }

    #[test]
    fn seal_contribution_order_insensitive() {
        // BTreeSet already sorts; this guards the render is sorted by key.
        let store = ProbeValueStore::new();
        store.insert("a", b"1\n".to_vec());
        store.insert("b", b"2\n".to_vec());
        let mut s1 = BTreeSet::new();
        s1.insert("a".to_string());
        s1.insert("b".to_string());
        let mut s2 = BTreeSet::new();
        s2.insert("b".to_string());
        s2.insert("a".to_string());
        assert_eq!(seal_contribution(&s1, &store), seal_contribution(&s2, &store));
    }

    #[test]
    fn distinct_keys_same_values_differ_from_swapped() {
        // Key bytes are part of the record, so {a=1,b=2} != {a=2,b=1}.
        let store = ProbeValueStore::new();
        store.insert("a", b"1".to_vec());
        store.insert("b", b"2".to_vec());
        let mut s = BTreeSet::new();
        s.insert("a".to_string());
        s.insert("b".to_string());
        let forward = seal_contribution(&s, &store);

        let swapped = ProbeValueStore::new();
        swapped.insert("a", b"2".to_vec());
        swapped.insert("b", b"1".to_vec());
        assert_ne!(forward, seal_contribution(&s, &swapped));
    }
}
