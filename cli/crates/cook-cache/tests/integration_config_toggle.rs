//! AC-Integ.3: Toggling between two configs (e.g. CXXFLAGS=-O0 ↔ -O3)
//! preserves both cache entries — no overwrite, no false hits, and
//! toggling back re-hits the prior entry.

use std::collections::BTreeMap;

use cook_cache::backend::{cloud_key, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend};
use cook_cache::envkey::{env_contribution, EnvDenylist};
use cook_cache::store::CACHE_VERSION;

fn key_for(env_contrib: u64) -> [u8; 32] {
    cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/Cookfile::build",
        command_hash: 0x1111,
        context_hash: 0x2222,
        env_contribution: env_contrib,
        sorted_input_content_hashes: &[0xaa, 0xbb],
    })
}

#[test]
fn toggling_cxxflags_produces_distinct_keys() {
    let denylist = EnvDenylist::baseline();

    let mut env_o2 = BTreeMap::new();
    env_o2.insert("CXXFLAGS".to_string(), "-O2".to_string());
    let env_contrib_o2 = env_contribution(&env_o2, &denylist);

    let mut env_o3 = BTreeMap::new();
    env_o3.insert("CXXFLAGS".to_string(), "-O3".to_string());
    let env_contrib_o3 = env_contribution(&env_o3, &denylist);

    assert_ne!(env_contrib_o2, env_contrib_o3);

    let key_o2 = key_for(env_contrib_o2);
    let key_o3 = key_for(env_contrib_o3);
    assert_ne!(key_o2, key_o3);
}

#[test]
fn toggling_back_rehits_prior_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let denylist = EnvDenylist::baseline();

    let mut env_o2 = BTreeMap::new();
    env_o2.insert("CXXFLAGS".to_string(), "-O2".to_string());
    let env_contrib_o2 = env_contribution(&env_o2, &denylist);
    let key_o2 = key_for(env_contrib_o2);

    let mut env_o3 = BTreeMap::new();
    env_o3.insert("CXXFLAGS".to_string(), "-O3".to_string());
    let env_contrib_o3 = env_contribution(&env_o3, &denylist);
    let key_o3 = key_for(env_contrib_o3);

    let meta_for = |env_c: u64| ArtifactMeta {
        recipe_namespace: "proj/Cookfile::build".into(),
        command_hash: 0x1111,
        context_hash: 0x2222,
        env_contribution: env_c,
        schema_version: CACHE_VERSION,
        size_bytes: 5,
        tags: Default::default(),
        consulted_env_keys: ["CXXFLAGS".to_string()].into_iter().collect(),
        output_index: 0,
        output_path: "build/main.o".into(),
    };

    backend.put(&key_o2, b"O2-bytes", &meta_for(env_contrib_o2)).expect("put o2");
    backend.put(&key_o3, b"O3-bytes", &meta_for(env_contrib_o3)).expect("put o3");

    // Toggle back: O2 still hits with O2 bytes (no overwrite).
    let bytes_o2 = backend.get(&key_o2).expect("get").expect("hit");
    assert_eq!(bytes_o2, b"O2-bytes");

    let bytes_o3 = backend.get(&key_o3).expect("get").expect("hit");
    assert_eq!(bytes_o3, b"O3-bytes");
}

#[test]
fn denylisted_env_does_not_change_key() {
    let denylist = EnvDenylist::baseline();

    // HOME is denylisted; toggling its value must not change env_contribution.
    let mut env_a = BTreeMap::new();
    env_a.insert("HOME".to_string(), "/home/alice".to_string());
    let mut env_b = BTreeMap::new();
    env_b.insert("HOME".to_string(), "/home/bob".to_string());

    let h_a = env_contribution(&env_a, &denylist);
    let h_b = env_contribution(&env_b, &denylist);
    assert_eq!(h_a, h_b);
}
