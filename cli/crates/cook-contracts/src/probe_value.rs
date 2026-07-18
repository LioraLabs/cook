//! Serialisation helpers for the probe-value store (§22.5.5).
//!
//! The canonical form for a probe value is **pretty-printed JSON with
//! bytewise-sorted object keys and exactly one trailing LF** (CS-0102).
//! Every path that persists or hashes a probe value — `.cook/probes/`,
//! the `CacheBackend` artifact body, and any content-hash input — uses
//! the bytes produced by [`encode_canonical_json`] verbatim.
//!
//! **Float note**: `-0.0` and `0.0` are `Value`-equal in serde_json but
//! encode to different canonical bytes (`"-0.0\n"` vs `"0.0\n"`). Because
//! hashing is byte-based this is fine — but callers must not deduplicate
//! probe values by `Value` equality before encoding.

use serde_json::Value as JsonValue;
use std::sync::atomic::{AtomicU64, Ordering};

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The reserved `produce` string of a `files { … }` probe (CS-0148). Not
/// executable Lua (a bare `@` is a syntax error), so no hand-written produce
/// body can collide with it. The engine intercepts a probe whose
/// `produce_source` equals this sentinel and synthesises its value from the
/// probe's resolved `inputs.files` — the same path→content-hash pairs the
/// fingerprint's FILES section folds — instead of dispatching a worker, so
/// the re-run trigger and the value can never drift.
pub const FILES_MANIFEST_PRODUCE: &str = "@files-manifest";

/// Build the canonical value bytes of a `files { … }` probe (CS-0148): a JSON
/// object mapping each workspace-relative path to the lowercase hex of its
/// content hash, or the literal `"<missing>"` when the file could not be read
/// (all-zero hash, mirroring §22.5.4's missing-file fold). Encoded via
/// [`encode_canonical_json`], so the bytes are store-canonical.
pub fn encode_files_manifest(files: &[(String, [u8; 32])]) -> Vec<u8> {
    let mut map = serde_json::Map::new();
    for (path, hash) in files {
        let v = if hash == &[0u8; 32] {
            JsonValue::String("<missing>".to_string())
        } else {
            let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
            JsonValue::String(hex)
        };
        map.insert(path.clone(), v);
    }
    encode_canonical_json(&JsonValue::Object(map))
}

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

/// Recursively rebuild a [`serde_json::Value`] with bytewise-sorted object
/// keys. Crate-internal; consumed by [`crate::member::member_to_string`]
/// so that compact member rendering shares the same key-ordering logic as the
/// canonical probe-value store (CS-0102).
///
/// Explicit so the canonical rendering is independent of serde_json's
/// `preserve_order` feature (additive; any future transitive dep could flip it).
pub(crate) fn canonical_value(v: &JsonValue) -> JsonValue {
    canonicalise(v)
}

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
/// artifact" — callers reading CacheBackend artifacts MUST treat an Err as
/// a miss (per §22.5.8). Worker-store reads (`cook.probes.get`) instead
/// surface decode failures as loud runtime errors: a hand-corrupted
/// `.cook/probes/<key>.json` should fail the unit, not silently rebuild.
pub fn decode_json(bytes: &[u8]) -> Result<JsonValue, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("probe-value JSON decode: {e}"))
}

/// File name for a probe key under `.cook/probes/`.
///
/// Uses a percent-style escape with `_` as the escape character so that the
/// mapping is **injective** (no two distinct keys produce the same file name):
///
/// | input char | encoded form |
/// |------------|--------------|
/// | `_`        | `_5f`        |
/// | `/`        | `_2f`        |
/// | `\`        | `_5c`        |
/// | everything else (incl. `:`) | literal |
///
/// Then `.json` is appended.  Keys that contain none of the three special
/// characters are unchanged — e.g. `cc:zlib` → `cc:zlib.json`.
///
/// Injectivity proof sketch: every `_` in the output is either an escaped
/// `_` (followed by `5f`) or the first byte of an escape sequence (followed
/// by `2f` or `5c`); a literal `_` cannot appear because every source `_`
/// is rewritten to `_5f`.  Therefore the decode is unambiguous and distinct
/// inputs cannot share an output.
///
/// **Platform caveat — `:` passes through literally.**  Colons are valid in
/// POSIX file names and the common `cc:zlib` style relies on this.  Windows
/// treats `:` as a drive-separator and rejects it in path components; Windows
/// support is deferred to SHI-176 Phase 5 and will require an additional
/// escape rule for `:`.
///
/// **Case-sensitivity caveat — injectivity is at the string level.**
/// On case-insensitive filesystems (macOS APFS default, Windows NTFS) two
/// probe keys that differ only by ASCII case — e.g. `CC:Zlib` vs `cc:zlib`
/// — would map to distinct file names but the OS may treat those names as
/// the same path, silently clobbering one entry.  Callers SHOULD normalise
/// probe keys to lowercase to avoid this on case-insensitive mounts.
pub fn probe_file_name(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 5);
    for ch in key.chars() {
        match ch {
            '_' => out.push_str("_5f"),
            '/' => out.push_str("_2f"),
            '\\' => out.push_str("_5c"),
            c => out.push(c),
        }
    }
    out.push_str(".json");
    out
}

/// Atomically materialise canonical probe bytes at `<dir>/<probe_file_name(key)>`
/// (write to a temp file in the same dir, then rename). Creates `dir` if absent.
///
/// The temp file name includes both the process id and a per-process
/// monotonic counter so that concurrent threads writing the same key do not
/// share a tmp path and tear each other's writes.
pub fn write_probe_file(
    dir: &std::path::Path,
    key: &str,
    bytes: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = probe_file_name(key);
    let final_path = dir.join(&name);
    let seq = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!(".{name}.tmp-{}-{seq}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp_path, bytes) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(final_path)
}

#[cfg(test)]
mod tests {
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

}
