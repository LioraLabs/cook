use super::*;
use serde_json::json;

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

#[test]
fn write_probe_file_creates_dir_and_writes_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("probes");
    let p = write_probe_file(&dir, "cc:zlib", b"42\n").unwrap();
    assert_eq!(p, dir.join("cc:zlib.json"));
    assert_eq!(std::fs::read(&p).unwrap(), b"42\n");
    // Overwrite goes through rename, not truncate-in-place.
    write_probe_file(&dir, "cc:zlib", b"43\n").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"43\n");
}
