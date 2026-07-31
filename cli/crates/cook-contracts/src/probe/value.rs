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
            JsonValue::String(crate::render::lower_hex(hash))
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

/// Merge a probe's per-run tool-path metadata into its value, producing the
/// READ VIEW (CS-0157): the view `cook.probes.get` returns and the view
/// `$<KEY.NAME.path>` substitution addresses (§22.5.7, CS-0192). Both MUST
/// see the same view, so both MUST build it through this one function.
///
/// The canonical value of a `tools { }` producer carries identity only
/// (`{ NAME = { hash } }`); the resolved path is location, recorded fresh
/// each run, so a consumer always reads where the tool resolves NOW and a
/// cached value can never replay a stale location.
///
/// The merge is shape-scoped: only an object entry under the tool's own name
/// that has a string `hash` member and no author-provided `path` member is
/// annotated, so custom-body probes that happen to declare `inputs.tools`
/// keep their values untouched.
pub fn merge_tool_paths(
    value: &mut JsonValue,
    tool_paths: &std::collections::BTreeMap<String, String>,
) {
    let JsonValue::Object(map) = value else {
        return;
    };
    for (tool, path) in tool_paths {
        if let Some(JsonValue::Object(entry)) = map.get_mut(tool) {
            let has_hash = matches!(entry.get("hash"), Some(JsonValue::String(_)));
            let path_absent = !entry.contains_key("path");
            if has_hash && path_absent {
                entry.insert("path".to_string(), JsonValue::String(path.clone()));
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/value_tests.rs"]
mod tests;
