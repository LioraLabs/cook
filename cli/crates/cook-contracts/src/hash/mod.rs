//! The workspace's stable 64-bit text fold (COOK-418).
//!
//! This crate's own constitution used `hash_str` as its worked example of a
//! law that could NOT live here: "law, but xxhash-bearing, its home is
//! `cook-fingerprint`". That reasoning conflated a dependency with an effect.
//! The bar `tests/layout.rs` enforces is `std::fs`, `std::env`,
//! `std::process`; xxhash computes and reaches nothing, so it clears the bar
//! as cleanly as `serde_json` does.
//!
//! The stratum invented to hold it did not exist, and the crate created to
//! occupy that stratum filled with the only thing adjacent to it: the IO half
//! of the cache.

/// Hash a string: command templates, env values, and every other place a
/// stable 64-bit fold over text is wanted.
///
/// xxh3 rather than a cryptographic hash because this is a local-cache fold,
/// not an integrity claim; the cloud key is SHA-256 (`cache::step`,
/// `envkey`).
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

#[cfg(test)]
#[path = "tests/hash_tests.rs"]
mod tests;
