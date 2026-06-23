//! AC-Integ.2: Two recipes producing `build/main.o` from different
//! sources/commands must produce different cloud keys.

use cook_cache::backend::{cloud_key, CloudKeyInputs};
use cook_cache::store::CACHE_VERSION;

#[test]
fn two_recipes_same_output_path_different_keys() {
    let inputs = [0u64, 1, 2];
    let key_a = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: 0xAA,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    let key_b = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::test",  // different recipe
        command_hash: 0xAA,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    assert_ne!(key_a, key_b, "different recipe → different cloud_key");
}

#[test]
fn cross_project_same_recipe_name_different_keys() {
    let inputs = [0u64];
    let key_a = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj-a/Cookfile::build",
        command_hash: 0xAA,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    let key_b = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj-b/Cookfile::build",
        command_hash: 0xAA,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    assert_ne!(key_a, key_b, "different project → different cloud_key");
}

#[test]
fn cross_cookfile_same_recipe_name_different_keys() {
    let inputs = [0u64];
    let key_a = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/Cookfile::build",
        command_hash: 0xAA,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    let key_b = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/services/api/Cookfile::build",
        command_hash: 0xAA,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    assert_ne!(key_a, key_b, "different sub-Cookfile → different cloud_key");
}
