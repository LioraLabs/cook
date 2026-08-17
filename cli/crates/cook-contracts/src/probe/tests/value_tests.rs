use super::{
    decode_json, encode_canonical_json, encode_files_manifest, encode_tools_identity,
    probe_file_name, FILES_MANIFEST_PRODUCE, TOOLS_IDENTITY_PRODUCE,
};
use serde_json::json;

// ── tools-identity tests (CS-0214) ──────────────────────────────────────

/// COOK-414's lesson, applied to the other value that crosses machines: a test
/// asserting only determinism or key order would stay green through a change of
/// wire format, and these bytes ARE the wire format — they are what
/// `seal_contribution` folds into every consuming unit's cache key.
///
/// Both digests were computed outside this codebase, so the assertion is
/// arithmetic rather than a recording of past behaviour. The preimages, so a
/// reader can re-derive them without cook:
///
/// ```text
/// printf 'cook COOK-416 tools golden\n' | sha256sum
///   == 83bb31602279150fde5cd041b9528f6bec5883e83715c49ffa0e50af6f0cde44
/// printf 'cook COOK-416 second tool\n'  | sha256sum
///   == 536f2e281c493bb330aa3bab2ede4cc0fd550ee2c74de443d5b0889d457c1195
/// ```
///
/// The expected byte string is also the exact content the PRE-CS-0214 Lua
/// producer wrote to `.cook/probes/<key>.json` (captured from a live run before
/// the interception landed), which is why CS-0214 moves no cache key.
const GOLDEN_CC: [u8; 32] = [
    0x83, 0xbb, 0x31, 0x60, 0x22, 0x79, 0x15, 0x0f, 0xde, 0x5c, 0xd0, 0x41, 0xb9, 0x52, 0x8f, 0x6b,
    0xec, 0x58, 0x83, 0xe8, 0x37, 0x15, 0xc4, 0x9f, 0xfa, 0x0e, 0x50, 0xaf, 0x6f, 0x0c, 0xde, 0x44,
];
const GOLDEN_LD: [u8; 32] = [
    0x53, 0x6f, 0x2e, 0x28, 0x1c, 0x49, 0x3b, 0xb3, 0x30, 0xaa, 0x3b, 0xab, 0x2e, 0xde, 0x4c, 0xc0,
    0xfd, 0x55, 0x0e, 0xe2, 0xc7, 0x4d, 0xe4, 0x43, 0xd5, 0xb0, 0x88, 0x9d, 0x45, 0x7c, 0x11, 0x95,
];

#[test]
fn the_tools_identity_encoding_is_these_exact_bytes() {
    let tools = vec![("cc".to_string(), GOLDEN_CC)];
    assert_eq!(
        String::from_utf8(encode_tools_identity(&tools)).unwrap(),
        "{\n  \"cc\": {\n    \"hash\": \
         \"83bb31602279150fde5cd041b9528f6bec5883e83715c49ffa0e50af6f0cde44\"\n  }\n}\n",
    );
}

#[test]
fn tools_identity_sorts_keys_bytewise() {
    // Two machines that declared the same tools in a different order must
    // still seal on the same bytes, so the value sorts by name.
    let forward = encode_tools_identity(&[
        ("cc".to_string(), GOLDEN_CC),
        ("ld".to_string(), GOLDEN_LD),
    ]);
    let reversed = encode_tools_identity(&[
        ("ld".to_string(), GOLDEN_LD),
        ("cc".to_string(), GOLDEN_CC),
    ]);
    assert_eq!(forward, reversed);
    let text = String::from_utf8(forward).unwrap();
    assert!(text.find("\"cc\"").unwrap() < text.find("\"ld\"").unwrap(), "{text}");
}

#[test]
fn tools_identity_carries_identity_only() {
    // CS-0157: the resolved path is location, not identity, and a location in
    // these bytes would key every sealing unit to a machine.
    let text = String::from_utf8(encode_tools_identity(&[("cc".to_string(), GOLDEN_CC)])).unwrap();
    assert!(!text.contains("path"), "path must never enter the value: {text}");
}

#[test]
fn tools_sentinel_is_not_valid_lua_and_is_its_own_sentinel() {
    // Same interception contract as `files`, and distinct from it: the two are
    // compared by equality in `cook-probe`, and a shared spelling would route
    // one producer kind's synthesis to the other's.
    assert!(TOOLS_IDENTITY_PRODUCE.starts_with('@'));
    assert_ne!(TOOLS_IDENTITY_PRODUCE, FILES_MANIFEST_PRODUCE);
}

// ── files-manifest tests (CS-0148) ──────────────────────────────────────

#[test]
fn files_manifest_sorts_keys_and_hex_encodes() {
    let files = vec![
        ("b.txt".to_string(), [0xabu8; 32]),
        ("a.txt".to_string(), [0x01u8; 32]),
    ];
    let text = String::from_utf8(encode_files_manifest(&files)).unwrap();
    let a = text.find("a.txt").unwrap();
    let b = text.find("b.txt").unwrap();
    assert!(a < b, "keys must sort bytewise: {text}");
    assert!(text.contains(&"ab".repeat(32)), "hex encoding: {text}");
    assert!(text.ends_with("}\n"), "canonical trailing LF: {text:?}");
}

#[test]
fn files_manifest_folds_missing_as_literal() {
    let files = vec![("gone.txt".to_string(), [0u8; 32])];
    let text = String::from_utf8(encode_files_manifest(&files)).unwrap();
    assert!(text.contains("<missing>"), "{text}");
}

#[test]
fn files_sentinel_is_not_valid_lua() {
    // The interception contract: no hand-written produce body can equal
    // the sentinel, because the sentinel cannot lex as Lua.
    assert!(FILES_MANIFEST_PRODUCE.starts_with('@'));
}

// ── canonical-JSON tests ─────────────────────────────────────────────────

#[test]
fn canonical_json_is_pretty_sorted_with_trailing_lf() {
    let v = json!({"b": 1, "a": [true, "x"]});
    let bytes = encode_canonical_json(&v);
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "{\n  \"a\": [\n    true,\n    \"x\"\n  ],\n  \"b\": 1\n}\n"
    );
}

#[test]
fn canonical_json_sorts_keys_recursively_and_bytewise() {
    let a = json!({"outer": {"zz": 1, "aa": 2}});
    let b = json!({"outer": {"aa": 2, "zz": 1}});
    assert_eq!(encode_canonical_json(&a), encode_canonical_json(&b));
}

#[test]
fn canonical_json_scalar_forms() {
    assert_eq!(encode_canonical_json(&json!(42)), b"42\n");
    assert_eq!(encode_canonical_json(&json!(null)), b"null\n");
    assert_eq!(encode_canonical_json(&json!("hi")), b"\"hi\"\n");
}

/// Pinned bytes for floats, large integers, and empty containers.
/// These must never silently change — a change here means the on-disk
/// format has shifted and old cache entries will hash differently.
#[test]
fn canonical_json_float_and_container_pinned_bytes() {
    assert_eq!(encode_canonical_json(&json!(1.0_f64)), b"1.0\n");
    assert_eq!(encode_canonical_json(&json!(0.1_f64)), b"0.1\n");
    assert_eq!(encode_canonical_json(&json!(-0.0_f64)), b"-0.0\n");
    assert_eq!(
        encode_canonical_json(&json!(18446744073709551615u64)),
        b"18446744073709551615\n"
    );
    // Empty object and array — pretty-printed form must be stable.
    assert_eq!(encode_canonical_json(&json!({})), b"{}\n");
    assert_eq!(encode_canonical_json(&json!([])), b"[]\n");
}

#[test]
fn decode_json_round_trips_canonical_bytes() {
    let v = json!({"found": true, "cflags": ["-I/usr/include"]});
    let bytes = encode_canonical_json(&v);
    assert_eq!(decode_json(&bytes).unwrap(), v);
}

#[test]
fn decode_json_rejects_pre_cs0102_bytes() {
    // 0x91 0xc3 is the old (pre-CS-0102) encoding of [true] — must be an
    // Err, the stale-artifact defence.
    assert!(decode_json(&[0x91, 0xc3]).is_err());
}

// ── probe_file_name tests ────────────────────────────────────────────────

#[test]
fn probe_file_name_escapes_path_separators() {
    // Unchanged: no special chars.
    assert_eq!(probe_file_name("cc:zlib"), "cc:zlib.json");
    // Both separators escaped; `_` itself also escaped.
    assert_eq!(probe_file_name("a/b\\c"), "a_2fb_5cc.json");
    // Underscore alone.
    assert_eq!(probe_file_name("a_b"), "a_5fb.json");
}

/// Injectivity: keys that previously collided under the old `__` scheme
/// now map to distinct file names.
#[test]
fn probe_file_name_is_injective() {
    // Old scheme: `a/b` → `a__b.json` and `a__b` → `a__b.json` (collision).
    // New scheme must differ.
    assert_ne!(probe_file_name("a/b"), probe_file_name("a__b"));

    // `a_b` and `a/b` must be distinct.
    assert_ne!(probe_file_name("a_b"), probe_file_name("a/b"));

    // All four of these keys must produce four distinct file names.
    let names: Vec<String> = ["a/b", "a__b", "a_b", "a_5fb"]
        .iter()
        .map(|k| probe_file_name(k))
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "duplicate file names: {names:?}");
}

// CS-0157 / CS-0192: the read view — canonical value plus per-run tool paths
// — is built by one function, so `cook.probes.get` and `$<KEY.NAME.path>`
// substitution can never disagree about it.

#[test]
fn merge_tool_paths_annotates_hash_bearing_entries() {
    let mut v = json!({"gcc": {"hash": "ab12"}, "ld": {"hash": "cd34"}});
    let paths = std::collections::BTreeMap::from([
        ("gcc".to_string(), "/usr/bin/gcc".to_string()),
    ]);
    super::merge_tool_paths(&mut v, &paths);
    assert_eq!(v["gcc"]["path"], json!("/usr/bin/gcc"));
    // No recorded path for ld: untouched.
    assert!(v["ld"].get("path").is_none());
}

#[test]
fn merge_tool_paths_is_shape_scoped() {
    // An author-provided path is never overwritten.
    let mut v = json!({"gcc": {"hash": "ab12", "path": "/custom/gcc"}});
    let paths = std::collections::BTreeMap::from([
        ("gcc".to_string(), "/usr/bin/gcc".to_string()),
    ]);
    super::merge_tool_paths(&mut v, &paths);
    assert_eq!(v["gcc"]["path"], json!("/custom/gcc"));

    // A hash-less entry is a custom-body value that happens to share a name:
    // untouched.
    let mut v = json!({"gcc": {"version": "13"}});
    super::merge_tool_paths(&mut v, &paths);
    assert!(v["gcc"].get("path").is_none());

    // A non-object root is untouched (and does not panic).
    let mut v = json!("scalar");
    super::merge_tool_paths(&mut v, &paths);
    assert_eq!(v, json!("scalar"));
}
