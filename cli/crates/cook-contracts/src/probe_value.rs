//! Serialisation helpers for the probe-value store (§22.5.5).
//!
//! The canonical form for a probe value is **pretty-printed JSON with
//! bytewise-sorted object keys and exactly one trailing LF** (CS-0102).
//! Every path that persists or hashes a probe value — `.cook/probes/`,
//! the `CacheBackend` artifact body, and any content-hash input — uses
//! the bytes produced by [`encode_canonical_json`] verbatim.
//!
//! The legacy msgpack helpers ([`encode_msgpack`] / [`decode_msgpack`]) are
//! retained while the codebase migrates; they will be removed once all call
//! sites have switched to the JSON codec.

use rmpv::Value as MsgPackValue;
use serde_json::Value as JsonValue;

/// Render a validated probe value (§22.5.5) to its canonical bytes:
/// pretty-printed JSON, 2-space indent, object keys sorted bytewise,
/// UTF-8, exactly one trailing LF. These bytes are the value's single
/// serialised form — the `.cook/probes/<key>.json` file, the CacheBackend
/// artifact body, and anything that hashes a probe value all use them
/// verbatim (CS-0102).
pub fn encode_canonical_json(v: &JsonValue) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(&canonicalise(v))
        .expect("serde_json pretty-print of a finite value tree cannot fail");
    s.push('\n');
    s.into_bytes()
}

/// Recursively rebuild objects with bytewise-sorted keys. Explicit so the
/// canonical rendering is independent of serde_json's `preserve_order`
/// feature (additive; any future transitive dep could flip it).
fn canonicalise(v: &JsonValue) -> JsonValue {
    match v {
        JsonValue::Array(items) => JsonValue::Array(items.iter().map(canonicalise).collect()),
        JsonValue::Object(map) => {
            let mut entries: Vec<(&String, &JsonValue)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            let mut out = serde_json::Map::new();
            for (k, val) in entries {
                out.insert(k.clone(), canonicalise(val));
            }
            JsonValue::Object(out)
        }
        other => other.clone(),
    }
}

/// Decode probe-value bytes. An Err here means "not a probe-value JSON
/// artifact" — callers on cache-read paths MUST treat it as a miss.
pub fn decode_json(bytes: &[u8]) -> Result<JsonValue, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("probe-value JSON decode: {e}"))
}

/// File name for a probe key under `.cook/probes/`: path separators are
/// replaced with `__`, then `.json` is appended. Everything else (incl.
/// `:`) stays literal — POSIX-only today.
pub fn probe_file_name(key: &str) -> String {
    format!("{}.json", key.replace(['/', '\\'], "__"))
}

/// Atomically materialise canonical probe bytes at `<dir>/<probe_file_name(key)>`
/// (write to a temp file in the same dir, then rename). Creates `dir` if absent.
pub fn write_probe_file(
    dir: &std::path::Path,
    key: &str,
    bytes: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = probe_file_name(key);
    let final_path = dir.join(&name);
    let tmp_path = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Encode an rmpv::Value to msgpack bytes.
pub fn encode_msgpack(v: &MsgPackValue) -> Vec<u8> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, v).expect("rmpv encode never fails for in-memory");
    buf
}

/// Decode msgpack bytes into an rmpv::Value.
pub fn decode_msgpack(bytes: &[u8]) -> Result<MsgPackValue, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    rmpv::decode::read_value(&mut cursor).map_err(|e| format!("msgpack decode: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::Value;
    use serde_json::json;

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

    #[test]
    fn decode_json_round_trips_canonical_bytes() {
        let v = json!({"found": true, "cflags": ["-I/usr/include"]});
        let bytes = encode_canonical_json(&v);
        assert_eq!(decode_json(&bytes).unwrap(), v);
    }

    #[test]
    fn decode_json_rejects_msgpack_bytes() {
        // Old msgpack [true] — must be an Err, the stale-artifact defence.
        assert!(decode_json(&[0x91, 0xc3]).is_err());
    }

    #[test]
    fn probe_file_name_escapes_path_separators() {
        assert_eq!(probe_file_name("cc:zlib"), "cc:zlib.json");
        assert_eq!(probe_file_name("a/b\\c"), "a__b__c.json");
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

    // ── msgpack round-trip tests (kept while old codec is still live) ────────

    #[test]
    fn encode_decode_bool() {
        let v = Value::Boolean(true);
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn encode_decode_integer() {
        let v = Value::Integer(42.into());
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn encode_decode_map() {
        let v = Value::Map(vec![
            (Value::String("found".into()), Value::Boolean(true)),
            (
                Value::String("cflags".into()),
                Value::Array(vec![Value::String("-I/usr/include".into())]),
            ),
        ]);
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn encode_decode_nil() {
        let v = Value::Nil;
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn decode_invalid_bytes_returns_error() {
        // Truncated msgpack — a map header claiming 1 element but no data.
        // 0x81 = fixmap with 1 entry; no key or value follows → decode error.
        let bad = vec![0x81];
        assert!(decode_msgpack(&bad).is_err());
    }
}
