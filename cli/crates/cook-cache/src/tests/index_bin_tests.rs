use super::*;
use crate::store::RecipeCache;
use cook_contracts::cache::observation::Observation;
use cook_contracts::cache::step::{FileRecord, StepEntry};
use std::collections::{BTreeMap, BTreeSet};

fn rec(path: &str, mtime: u64, hash: u64) -> FileRecord {
    FileRecord { path: path.into(), mtime, hash }
}

fn step(inputs: Vec<FileRecord>, outputs: Vec<FileRecord>) -> StepEntry {
    StepEntry {
        inputs,
        outputs,
        command_hash: 0x0102030405060708,
        env_contribution: 0x2222222222222222,
        seal_contribution: 0x3333333333333333,
        observed: None,
    }
}

/// Two steps deliberately share `src/common.h` so the interning tests have
/// something to observe.
fn populated() -> RecipeCache {
    let mut cache = RecipeCache::new();

    let mut globs = BTreeMap::new();
    globs.insert(
        "src/*.c".to_string(),
        BTreeSet::from(["src/main.c".to_string(), "src/util.c".to_string()]),
    );
    globs.insert("docs/*.md".to_string(), BTreeSet::new());
    cache.globs = globs;

    cache.steps.insert(
        "compile_main".to_string(),
        step(
            vec![rec("src/main.c", 1_700_000_000, 0x1234567890abcdef),
                 rec("src/common.h", 1_700_000_050, 0x5555555555555555)],
            vec![rec("build/main.o", 1_700_000_100, 0xabcdef1234567890)],
        ),
    );
    cache.steps.insert(
        "compile_util".to_string(),
        step(
            vec![rec("src/util.c", 1_700_000_001, 0xfedcba9876543210),
                 rec("src/common.h", 1_700_000_050, 0x5555555555555555)],
            vec![rec("build/util.o", 1_700_000_101, 0x0f0f0f0f0f0f0f0f)],
        ),
    );
    cache
}

#[test]
fn empty_cache_round_trips() {
    let original = RecipeCache::new();
    let bytes = encode(&original);
    let restored = decode(&bytes).expect("decode");
    assert_eq!(original, restored);
    assert_eq!(restored.schema_version, CACHE_VERSION);
}

#[test]
fn populated_cache_round_trips() {
    let original = populated();
    let restored = decode(&encode(&original)).expect("decode");
    assert_eq!(original, restored);
}

#[test]
fn step_with_no_outputs_round_trips() {
    let mut cache = RecipeCache::new();
    cache.steps.insert(
        "probe_only".to_string(),
        step(vec![rec("src/main.c", 1, 2)], vec![]),
    );
    assert_eq!(cache, decode(&encode(&cache)).expect("decode"));
}

#[test]
fn step_observation_round_trips() {
    let mut cache = RecipeCache::new();
    let mut observed = step(vec![rec("src/main.c", 1, 2)], vec![]);
    observed.observed =
        Some(Observation::new(1_500, 1_753_600_000, Some("input changed: src/main.c".into()), 42));
    cache.steps.insert("observed".to_string(), observed);
    assert_eq!(cache, decode(&encode(&cache)).expect("decode"));
}

#[test]
fn step_with_no_inputs_round_trips() {
    let mut cache = RecipeCache::new();
    cache.steps.insert(
        "generated".to_string(),
        step(vec![], vec![rec("build/out.txt", 3, 4)]),
    );
    assert_eq!(cache, decode(&encode(&cache)).expect("decode"));
}

#[test]
fn non_ascii_paths_round_trip() {
    let mut cache = RecipeCache::new();
    cache.steps.insert(
        "unicode".to_string(),
        step(
            vec![rec("src/café/naïve — ünïcode.c", 5, 6)],
            vec![rec("build/日本語.o", 7, 8)],
        ),
    );
    assert_eq!(cache, decode(&encode(&cache)).expect("decode"));
}

#[test]
fn recipe_keys_with_env_suffixes_round_trip() {
    // Real cache keys carry `@<hash>` and `@<env:...>` suffixes.
    let mut cache = RecipeCache::new();
    cache.steps.insert(
        "build/obj/duckdb_lib/src_common_bignum_cpp.o@2d06800538d394c2".to_string(),
        step(vec![rec("src/common/bignum.cpp", 9, 10)], vec![]),
    );
    cache.steps.insert(
        "build/obj/x.o@env:PROFILE=release".to_string(),
        step(vec![], vec![]),
    );
    assert_eq!(cache, decode(&encode(&cache)).expect("decode"));
}

#[test]
fn glob_with_empty_member_set_round_trips() {
    let mut cache = RecipeCache::new();
    cache.globs.insert("nothing/*.zz".to_string(), BTreeSet::new());
    assert_eq!(cache, decode(&encode(&cache)).expect("decode"));
}

#[test]
fn shared_path_is_stored_once() {
    // `src/common.h` appears in both steps' inputs. The blob must hold one
    // copy: this is the 77x-redundancy win the whole format exists for.
    let bytes = encode(&populated());
    let needle = b"src/common.h";
    let occurrences = bytes
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count();
    assert_eq!(occurrences, 1, "path blob must intern shared paths");
}

#[test]
fn decoded_records_share_one_allocation_per_path() {
    // The whole point of interning: `src/common.h` is named by a record in
    // each of the two steps, and after decoding both records must point at the
    // SAME allocation. Without this, loading an index allocates once per
    // record (328k times on DuckDB) instead of once per distinct path (6,730).
    let decoded = decode(&encode(&populated())).expect("decode");
    let main = &decoded.steps["compile_main"].inputs;
    let util = &decoded.steps["compile_util"].inputs;

    let a = main.iter().find(|r| &*r.path == "src/common.h").expect("in compile_main");
    let b = util.iter().find(|r| &*r.path == "src/common.h").expect("in compile_util");
    assert!(
        std::sync::Arc::ptr_eq(&a.path, &b.path),
        "records naming the same path must share one Arc"
    );

    // Distinct paths must NOT be conflated into one allocation.
    let distinct = main.iter().find(|r| &*r.path == "src/main.c").expect("main.c");
    assert!(!std::sync::Arc::ptr_eq(&a.path, &distinct.path));
}

#[test]
fn encoding_is_deterministic() {
    // update_step's equality short-circuit and the dirty-tracking model both
    // assume an unchanged cache re-encodes to the same bytes.
    let cache = populated();
    assert_eq!(encode(&cache), encode(&cache));

    // Insertion order must not leak into the encoding either.
    let mut a = RecipeCache::new();
    a.steps.insert("z".to_string(), step(vec![rec("b.c", 1, 2)], vec![]));
    a.steps.insert("a".to_string(), step(vec![rec("a.c", 3, 4)], vec![]));
    let mut b = RecipeCache::new();
    b.steps.insert("a".to_string(), step(vec![rec("a.c", 3, 4)], vec![]));
    b.steps.insert("z".to_string(), step(vec![rec("b.c", 1, 2)], vec![]));
    assert_eq!(encode(&a), encode(&b));
}

#[test]
fn header_is_the_documented_shape() {
    let bytes = encode(&populated());
    assert_eq!(&bytes[0..8], MAGIC);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), CACHE_VERSION);
    assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 0);
    let payload_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    assert_eq!(payload_len as usize, bytes.len() - HEADER_LEN);
}

#[test]
fn bad_magic_is_refused() {
    let mut bytes = encode(&populated());
    bytes[0] = b'X';
    assert!(matches!(decode(&bytes), Err(DecodeError::BadMagic)));
}

#[test]
fn truncated_header_is_refused() {
    let bytes = encode(&populated());
    assert!(matches!(decode(&bytes[..16]), Err(DecodeError::Truncated)));
    assert!(matches!(decode(&[]), Err(DecodeError::Truncated)));
}

#[test]
fn truncated_payload_is_refused() {
    let bytes = encode(&populated());
    let cut = &bytes[..bytes.len() - 8];
    assert!(matches!(decode(cut), Err(DecodeError::Truncated)));
}

#[test]
fn flipped_payload_byte_is_refused_by_checksum() {
    let mut bytes = encode(&populated());
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert!(matches!(decode(&bytes), Err(DecodeError::ChecksumMismatch)));
}

#[test]
fn wrong_schema_version_is_refused() {
    // Both directions: a file from an older cook, and one from a future cook
    // whose layout this binary cannot reason about. Expressed relative to
    // CACHE_VERSION so the test keeps its meaning across future bumps.
    let older = CACHE_VERSION - 1;
    let mut bytes = encode(&populated());
    bytes[8..12].copy_from_slice(&older.to_le_bytes());
    assert_eq!(decode(&bytes), Err(DecodeError::SchemaVersion(older)));

    let newer = CACHE_VERSION + 1;
    let mut bytes = encode(&populated());
    bytes[8..12].copy_from_slice(&newer.to_le_bytes());
    assert_eq!(decode(&bytes), Err(DecodeError::SchemaVersion(newer)));
}

#[test]
fn corrupt_internal_offset_errors_without_panicking() {
    // The checksum normally catches this, so recompute it after corrupting so
    // the bounds checks themselves are what has to hold. Every u32 in the
    // payload is walked; any one of them pointing out of range must produce a
    // clean Err rather than a panic or an out-of-bounds read.
    let base = encode(&populated());
    let mut refused = 0;
    for offset in (HEADER_LEN..base.len() - 4).step_by(4) {
        let mut bytes = base.clone();
        bytes[offset..offset + 4].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        reseal(&mut bytes);
        match decode(&bytes) {
            Err(_) => refused += 1,
            // A few u32 slots are mtime/hash halves where any value is legal.
            Ok(_) => {}
        }
    }
    assert!(refused > 0, "expected some corrupted offsets to be refused");
}

#[test]
fn zero_length_payload_is_refused() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&xxhash_rust::xxh3::xxh3_64(&[]).to_le_bytes());
    assert!(decode(&bytes).is_err());
}

/// Recompute the header's payload checksum after a test mutates the payload.
fn reseal(bytes: &mut [u8]) {
    let hash = xxhash_rust::xxh3::xxh3_64(&bytes[HEADER_LEN..]);
    bytes[24..32].copy_from_slice(&hash.to_le_bytes());
}
