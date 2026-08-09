//! The identity a unit's cache entry is filed under (§17.1.1.1, CS-0186).
//!
//! Pure: a function of what the author declared, and nothing else. It lived in
//! cook-register until COOK-421, where it was the crate's only reason to
//! depend on `xxhash-rust` — hashing law placed in the phase that happened to
//! call it, while `crate::cache::record`'s doc comment already reached back to
//! name `build_local_cache_key` and `OBSERVING_KEY_MARKER` from here.
//!
//! It is one decision with more than one surface that must agree on it: the
//! register phase composes the key, the executor files and replays records
//! under it, and `cook why` explains a miss in terms of it. A second
//! implementation would not fail a build — it would file a unit under an
//! identity nothing else looks for, which reads as a permanent miss.

/// The marker that opens an observing unit's identity (§17.1.1.1, CS-0186).
///
/// Producing keys are a declared output path, optionally suffixed with the env
/// contribution. Observing keys are a digest, and the two share one index, so
/// the digest is written in a form no declared output path takes. `:` is not a
/// path separator on any supported platform and a workspace-relative output
/// beginning with one is not a path any Cookfile writes.
///
/// A collision would not be a correctness failure — the two units' output
/// counts disagree, so each is judged stale against the other's record and
/// rebuilds rather than replaying it — but they would then clobber each other
/// on every run, which is the permanent-churn shape CS-0169 exists to refuse.
/// Nothing else refuses it for us: `reject_duplicate_outputs` compares DECLARED
/// OUTPUTS, and an observing unit declares none, so this marker is the whole of
/// what keeps the two spaces apart.
pub const OBSERVING_KEY_MARKER: char = ':';

/// Build a local cache key that encodes env_contribution so simultaneous
/// variant builds (e.g. different env-selected toolchains) coexist without
/// overwriting each other.
///
/// Two shapes, one convention: `<identity>` or `<identity>@<env-hex>`. What
/// serves as the identity is what the unit's effect kind (§17.1.1.1) leaves
/// available.
pub fn build_local_cache_key(
    _cookfile_path: &str,
    _recipe: &str,
    output_paths: &[String],
    inputs: &[crate::cache::DeclaredInput],
    command_hash: u64,
    env_contribution: u64,
    seal_keys: &std::collections::BTreeSet<String>,
) -> String {
    let identity = match output_paths.first() {
        // A producing unit is identified by a declared output path, which
        // CS-0169 makes claimable by at most one unit.
        Some(first) => first.clone(),
        // An observing unit has no output path, so it is identified by a
        // digest of its DECLARATION (CS-0186).
        None => observing_identity(inputs, command_hash, seal_keys),
    };
    if env_contribution != 0 {
        format!("{identity}@{env_contribution:x}")
    } else {
        identity
    }
}

/// The identity of a unit that declares no outputs: a digest over the paths it
/// declares and the command it runs.
///
/// Three exclusions are normative (§17.1.1.1), and each is load-bearing:
///
/// * **Not the input CONTENTS.** This is the one that decides the design. An
///   identity that moved whenever the contents moved would never be found
///   twice, so no prior record would ever be available to compare against,
///   every invalidation would present as a first-ever build, and `cook why`
///   could report a key but never a cause. Identity says WHICH UNIT; the
///   recorded determinants and input records say WHETHER IT MAY BE REPLAYED.
/// * **Not the recipe name, and not the source position.** §17.4 requires that
///   moving a test within a recipe, or a recipe between Cookfiles, not bust its
///   cache. Both are already in scope here — `build_local_cache_key` takes
///   `_cookfile_path` and `_recipe` and has never used either — and they stay
///   unused deliberately rather than by omission.
/// * **Nothing further than the determinants the declaration carries.** Two
///   units in one index that no such determinant separates share one record,
///   and replaying either for the other cannot be observed.
///
///   The **effective seal key set** is one of those determinants, which is why
///   it is hashed here. Sealed VALUES cannot be — they are execute-phase, and
///   this runs at register phase — but leaving the KEYS out too makes
///   `test { ./run } seal toolchain` and a bare `test { ./run }` in one recipe
///   one identity. They then share a record whose recorded seal contribution
///   matches at most one of them, so each invalidates the other on every run:
///   not a false green, but the permanent churn CS-0169 exists to refuse.
///

/// What it replaces: `<first-input>@<command-hash>`. That form was weakly
/// unique — every unit sharing a first input and a command text collided, and
/// a unit declaring NO inputs keyed as the empty string plus its command hash,
/// so two such units in one recipe were one entry. It was nearly unreachable
/// while output-less units were nearly never cached, and CS-0186 makes it
/// carry every test unit in the project.
///
/// Paths are deduplicated, order-preserving, for the same reason the test
/// payload's input list is (COOK-84): a path named by both `inputs` and a
/// step-group dep arrived twice, and a unit's identity must not depend on how
/// many ways a file was reached. Order is otherwise kept as declared, which is
/// deterministic per registration; sorting would additionally erase a
/// reordering that IS a declaration change.
fn observing_identity(
    inputs: &[crate::cache::DeclaredInput],
    command_hash: u64,
    seal_keys: &std::collections::BTreeSet<String>,
) -> String {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut seen: Vec<&str> = Vec::with_capacity(inputs.len());
    for entry in inputs {
        let p = entry.path.as_str();
        if seen.contains(&p) {
            continue;
        }
        seen.push(p);
        // NUL-terminated so ["ab", "c"] and ["a", "bc"] cannot hash alike;
        // a path cannot contain NUL, so the separator is unambiguous. The kind
        // rides along because it is part of what was declared: the same string
        // read as a file and read as a pattern are two different declarations
        // (§17.1.1.2).
        hasher.update(p.as_bytes());
        hasher.update(if entry.is_pattern() { b"\0*" } else { b"\0=" });
    }
    hasher.update(&command_hash.to_le_bytes());
    // The seal key set, sorted by the BTreeSet it arrives in: the same keys
    // declared in a different order are the same declaration.
    for key in seal_keys {
        hasher.update(key.as_bytes());
        hasher.update(b"\0");
    }
    format!("{}{:016x}", OBSERVING_KEY_MARKER, hasher.digest())
}

#[cfg(test)]
#[path = "tests/local_key_tests.rs"]
mod tests;
