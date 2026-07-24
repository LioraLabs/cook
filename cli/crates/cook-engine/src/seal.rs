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
#[path = "tests/seal_tests.rs"]
mod tests;
