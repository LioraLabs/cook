use super::{
    decode_json, encode_canonical_json, encode_files_manifest, encode_tools_identity,
    probe_delta, probe_file_name, ProbeDelta, FILES_MANIFEST_PRODUCE, TOOLS_IDENTITY_PRODUCE,
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

// ── probe_delta tests (CS-0245) ──────────────────────────────────────────

#[test]
fn probe_delta_is_none_for_byte_identical_values() {
    let bytes = encode_canonical_json(&json!({"a": 1}));
    assert_eq!(probe_delta(Some(&bytes), &bytes), None);
}

#[test]
fn probe_delta_reports_first_observation_with_no_prior() {
    let current = encode_canonical_json(&json!("x86_64-linux"));
    assert_eq!(probe_delta(None, &current), Some(ProbeDelta::FirstObservation));
}

#[test]
fn probe_delta_names_added_removed_and_changed_files_manifest_entries() {
    let prior = encode_files_manifest(&[
        ("a.ts".to_string(), [0x01u8; 32]),
        ("b.ts".to_string(), [0x02u8; 32]),
    ]);
    let current = encode_files_manifest(&[
        ("a.ts".to_string(), [0x01u8; 32]), // unchanged — must NOT be named
        ("b.ts".to_string(), [0x03u8; 32]), // changed
        ("c.ts".to_string(), [0x04u8; 32]), // added
    ]);
    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    let ProbeDelta::Entries { added, removed, changed } = delta else {
        panic!("expected Entries, got {delta:?}");
    };
    assert_eq!(added, vec![("c.ts".to_string(), "04".repeat(32))]);
    assert_eq!(removed, Vec::<(String, String)>::new());
    assert_eq!(
        changed,
        vec![("b.ts".to_string(), "02".repeat(32), "03".repeat(32))]
    );
}

#[test]
fn probe_delta_names_a_removed_files_manifest_entry() {
    let prior = encode_files_manifest(&[
        ("a.ts".to_string(), [0x01u8; 32]),
        ("gone.ts".to_string(), [0x02u8; 32]),
    ]);
    let current = encode_files_manifest(&[("a.ts".to_string(), [0x01u8; 32])]);
    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    let ProbeDelta::Entries { added, removed, changed } = delta else {
        panic!("expected Entries, got {delta:?}");
    };
    assert!(added.is_empty());
    assert!(changed.is_empty());
    assert_eq!(removed, vec![("gone.ts".to_string(), "02".repeat(32))]);
}

#[test]
fn probe_delta_names_a_moved_tools_identity_hash_both_renderings() {
    let cc_old = [0x01u8; 32];
    let cc_new = [0x02u8; 32];
    let prior = encode_tools_identity(&[("cc".to_string(), cc_old)]);
    let current = encode_tools_identity(&[("cc".to_string(), cc_new)]);
    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    let ProbeDelta::Entries { added, removed, changed } = delta else {
        panic!("expected Entries, got {delta:?}");
    };
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert_eq!(changed.len(), 1);
    let (name, old, new) = &changed[0];
    assert_eq!(name, "cc");
    assert!(old.contains(&"01".repeat(32)), "{old}");
    assert!(new.contains(&"02".repeat(32)), "{new}");
}

#[test]
fn probe_delta_is_value_for_a_scalar() {
    let prior = encode_canonical_json(&json!("x86_64-linux"));
    let current = encode_canonical_json(&json!("aarch64-linux"));
    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    assert!(matches!(delta, ProbeDelta::Value { .. }), "{delta:?}");
}

#[test]
fn probe_delta_is_value_for_an_object_vs_scalar_shape_change() {
    let prior = encode_canonical_json(&json!({"a": 1}));
    let current = encode_canonical_json(&json!("scalar"));
    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    assert!(matches!(delta, ProbeDelta::Value { .. }), "{delta:?}");
}

/// Pins the fix in `object_entries_diff`: `-0.0` and `0.0` are `Value`-equal
/// in serde_json (see this module's header) but encode to different
/// canonical bytes, and the seal fold hashes bytes. A `changed`-detection
/// rule built on `JsonValue::PartialEq` would silently drop this entry from
/// `changed` even though the unit's `seal_contribution` DID move — the
/// query would then report a seal-changed miss naming no probe, which is
/// exactly the "empty diff presented as the full account" §17.1.6.1
/// forbids for a case that in fact has an entry to name.
#[test]
fn probe_delta_names_a_changed_entry_across_a_value_equal_float_pair() {
    let prior = encode_canonical_json(&json!({"x": 0.0}));
    let current = encode_canonical_json(&json!({"x": -0.0}));
    // Sanity: this is genuinely the `Value`-equal-but-byte-different case the
    // module header warns about, or this test pins nothing.
    assert_eq!(json!(0.0_f64), json!(-0.0_f64), "test premise: Value-equal");
    assert_ne!(prior, current, "test premise: byte-different");

    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    let ProbeDelta::Entries { added, removed, changed } = delta else {
        panic!("expected Entries, got {delta:?}");
    };
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert_eq!(changed, vec![("x".to_string(), "0.0".to_string(), "-0.0".to_string())]);
}

/// CS-0245 quoting consistency: a top-level scalar string renders unquoted in
/// `ProbeDelta::Value`, the same way a string entry renders unquoted in
/// `ProbeDelta::Entries` — a `host`-style probe is the case a user sees.
#[test]
fn probe_delta_value_unquotes_a_scalar_string_like_entries_does() {
    let prior = encode_canonical_json(&json!("x86_64-linux"));
    let current = encode_canonical_json(&json!("aarch64-linux"));
    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    let ProbeDelta::Value { old, new } = delta else {
        panic!("expected Value, got {delta:?}");
    };
    assert_eq!(old, "x86_64-linux", "must not carry JSON quotes: {old:?}");
    assert_eq!(new, "aarch64-linux", "must not carry JSON quotes: {new:?}");
}

/// M-3 (seam review on 9eb2faf7): the entry-comparison fix that closed the
/// `-0.0`/`0.0` gap above opened a smaller one. `render_entry` unquotes a
/// JSON string so a hash reads as a hash, but comparing the RENDERED forms
/// for `changed`-detection meant a bool `true` and a string `"true"` — two
/// different types, byte-different canonical encodings — rendered to the
/// identical text `true` and so compared equal, dropping a real entry out of
/// `changed`. A unit sealing this key would then report a seal-changed miss
/// naming no probe, exactly the failure `probe_delta_names_a_changed_entry_
/// across_a_value_equal_float_pair` above claims to pin for the float case.
#[test]
fn probe_delta_names_a_bool_vs_string_changed_entry() {
    let prior = encode_canonical_json(&json!({"x": true}));
    let current = encode_canonical_json(&json!({"x": "true"}));
    assert_ne!(prior, current, "test premise: byte-different");

    let delta = probe_delta(Some(&prior), &current).expect("bytes moved");
    let ProbeDelta::Entries { added, removed, changed } = delta else {
        panic!("expected Entries, got {delta:?}");
    };
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert_eq!(
        changed,
        vec![("x".to_string(), "true".to_string(), "true".to_string())],
        "a bool and a same-spelling string must still be named as changed"
    );
}
