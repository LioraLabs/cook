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

/// The reserved `produce` string of a top-level `files` declaration (CS-0148). Not
/// executable Lua (a bare `@` is a syntax error), so no hand-written produce
/// body can collide with it. The engine intercepts a probe whose
/// `produce_source` equals this sentinel and synthesises its value from the
/// probe's resolved `inputs.files` — the same path→content-hash pairs the
/// declaration resolves — instead of dispatching a worker, so the resolution
/// and the value can never drift.
pub const FILES_MANIFEST_PRODUCE: &str = "@files-manifest";

/// Build the canonical value bytes of a top-level `files` declaration (CS-0148): a JSON
/// object mapping each workspace-relative path to the lowercase hex of its
/// content hash, or the literal `"<missing>"` when the file could not be read
/// (all-zero hash, mirroring §22.5.4's missing-file rule). Encoded via
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

/// The reserved `produce` string of a top-level `tools` declaration (CS-0214). Same
/// interception contract as [`FILES_MANIFEST_PRODUCE`] and deliberately a
/// different spelling: the two are compared by equality, so one string could
/// not stand for both without routing one producer kind's synthesis to the
/// other's.
///
/// Before CS-0214 the `tools` lowering emitted a Lua program that resolved each
/// name with `command -v` and digested it with `sha256sum`, while the engine's
/// own resolution used `which` and `sha2`. Two machineries
/// answering one question, at two different moments in the run, agreeing only
/// because both happened to land on lowercase-hex SHA-256 — and the emitted one
/// could not run at all on a host without GNU coreutils.
pub const TOOLS_IDENTITY_PRODUCE: &str = "@tools-identity";

/// Build the canonical value bytes of a top-level `tools` declaration (CS-0214): a JSON
/// object keyed by tool name, each entry `{ "hash": "<lowercase hex>" }`.
///
/// The pairs are the probe's resolved `inputs.tools`, so the digests the
/// declaration resolves and the digests the value carries are one computation
/// rather than two that agree. Keys sort bytewise via
/// [`encode_canonical_json`].
///
/// Identity only: the resolved PATH location is deliberately absent (CS-0157).
/// A location in these bytes would fold into every sealing unit's key through
/// `seal_contribution`, so identical toolchains installed at different prefixes
/// could never share a sealed artifact. Path reaches consumers through the
/// per-run read view instead ([`merge_tool_paths`]).
///
/// Total by construction: a name that does not resolve on PATH MUST fail the
/// probe by name (§22.5.2) before reaching here, and that rejection cannot be
/// made here — an all-zero digest means "could not read the bytes", which a
/// present-but-unreadable binary also produces.
pub fn encode_tools_identity(tools: &[(String, [u8; 32])]) -> Vec<u8> {
    let mut map = serde_json::Map::new();
    for (name, hash) in tools {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "hash".to_string(),
            JsonValue::String(crate::render::lower_hex(hash)),
        );
        map.insert(name.clone(), JsonValue::Object(entry));
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
/// The canonical value of a top-level `tools` declaration carries identity only
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

/// How a probe's value moved between two observations (CS-0245,
/// §{exec.cache.why.determinants}): the single law both the local-miss
/// attribution and the shared-miss `DeterminantDiff::Probe` rendering read
/// through, so the two tiers cannot name a moved probe differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeDelta {
    /// No prior recorded value for this key.
    FirstObservation,
    /// Both sides decode to JSON objects: per-top-level-key differences.
    /// `(entry key, rendered value)` for added/removed; `(entry key, old,
    /// new)` for changed. Each list is sorted by entry key.
    Entries {
        added: Vec<(String, String)>,
        removed: Vec<(String, String)>,
        changed: Vec<(String, String, String)>,
    },
    /// Anything else: the whole values, compactly rendered.
    Value { old: String, new: String },
}

/// The delta between a probe's prior recorded value and its current one, or
/// `None` when the value did not move.
///
/// - `None` when `prior == Some(current)` **byte-for-byte** — byte equality,
///   not `serde_json::Value` equality (`-0.0` vs `0.0`, see the module
///   header), because this is what the seal fold hashes.
/// - `prior == None` is [`ProbeDelta::FirstObservation`]: there is nothing to
///   diff against, and this function never fabricates an old value.
/// - [`ProbeDelta::Entries`] when BOTH sides decode via [`decode_json`] to
///   JSON objects — top-level granularity only, which covers a `files`
///   manifest (`path -> "hex"`) and a `tools` identity (`name ->
///   {"hash": "hex"}`) under one rule. Each entry value is rendered with
///   [`serde_json::to_string`] (compact, single line), except that a JSON
///   string renders unquoted so a hash reads as a hash.
/// - [`ProbeDelta::Value`] otherwise, including when either side fails to
///   decode (rendered lossily) — a probe value is canonical JSON, so this is
///   the defensive arm.
///
/// Pure: no I/O, no `Path`, no logging.
pub fn probe_delta(prior: Option<&[u8]>, current: &[u8]) -> Option<ProbeDelta> {
    let prior = match prior {
        None => return Some(ProbeDelta::FirstObservation),
        Some(p) => p,
    };
    if prior == current {
        return None;
    }
    match (decode_json(prior), decode_json(current)) {
        (Ok(JsonValue::Object(po)), Ok(JsonValue::Object(co))) => {
            Some(object_entries_diff(po, co))
        }
        _ => Some(ProbeDelta::Value {
            old: render_compact(prior),
            new: render_compact(current),
        }),
    }
}

/// Top-level-only diff of two JSON objects, sorted by entry key.
///
/// Collected into `BTreeMap`s rather than trusting `serde_json::Map`'s own
/// iteration order: that order is insertion order unless the crate's
/// `preserve_order` feature is off, in which case it is already sorted — this
/// function must not depend on which is true for whatever build enabled it
/// transitively (see the module header's determinism note).
fn object_entries_diff(
    prior: serde_json::Map<String, JsonValue>,
    current: serde_json::Map<String, JsonValue>,
) -> ProbeDelta {
    let prior: std::collections::BTreeMap<String, JsonValue> = prior.into_iter().collect();
    let current: std::collections::BTreeMap<String, JsonValue> = current.into_iter().collect();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let keys: std::collections::BTreeSet<&String> = prior.keys().chain(current.keys()).collect();
    for k in keys {
        match (prior.get(k), current.get(k)) {
            (None, Some(v)) => added.push((k.clone(), render_entry(v))),
            (Some(v), None) => removed.push((k.clone(), render_entry(v))),
            (Some(a), Some(b)) => {
                // M-3 (seam review on 9eb2faf7): compare the CANONICAL form
                // (`serde_json::to_string`, which stays typed and quoted),
                // not the rendered DISPLAY form `render_entry` produces.
                // Comparing the rendered strings closed the `-0.0`/`0.0` gap
                // the module header warns about but opened a smaller one of
                // the same shape: `render_entry` unquotes a JSON string, so
                // the bool `true` and the string `"true"` — or the number
                // `0.0` and the string `"0.0"` — rendered identically and a
                // byte-different, type-different entry silently dropped out
                // of `changed`. Compare the typed encoding; render the
                // display form separately, once, for the caller.
                let (ca, cb) = (
                    serde_json::to_string(a).unwrap_or_default(),
                    serde_json::to_string(b).unwrap_or_default(),
                );
                if ca != cb {
                    changed.push((k.clone(), render_entry(a), render_entry(b)));
                }
            }
            _ => {}
        }
    }
    ProbeDelta::Entries { added, removed, changed }
}

/// One entry value, compact — a JSON string unwraps to its bare text so a
/// hash or a path reads as itself rather than as a quoted JSON literal.
fn render_entry(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Compact rendering of a whole probe value for [`ProbeDelta::Value`]. Valid
/// canonical JSON renders through [`render_entry`] — the same function
/// `Entries` renders each entry with — so a top-level JSON string unwraps to
/// its bare text exactly as an entry value does; anything else renders as
/// compact JSON. A value that fails to decode (the defensive arm
/// `probe_delta` exists for) renders as lossy UTF-8 with the canonical
/// trailing LF trimmed.
///
/// **Quoting consistency (deliberate, not an oversight):** an earlier version
/// of this function kept JSON quotes on a top-level string (`"x86_64-linux"`)
/// while `Entries`' `render_entry` unwrapped a string entry to bare text
/// (`hex...` not `"hex..."`). A scalar probe — a `host` string is the case a
/// user actually sees in a `ProbeDelta::Value` — should read the same way
/// whether it arrived as a whole-value scalar or as one entry of an object,
/// so both go through `render_entry` now. There is no case where the two
/// rendering styles need to differ, and the DISPLAY form this pair produces
/// is not the thing compared for equality — `probe_delta`'s equality check
/// (and `object_entries_diff`'s per-entry one, M-3) is over the canonical
/// encoding, so unwrapping a string here for readability cannot mask a real
/// difference. It IS, however, machine-parsed downstream: `cook-cli`'s JSON
/// renderer (`why_render.rs::probe_delta_json`) emits exactly these strings
/// verbatim as its `old`/`new`/`value` fields, so a `cook why --format json`
/// consumer parses this display form, and a change here changes that wire
/// shape.
fn render_compact(bytes: &[u8]) -> String {
    match decode_json(bytes) {
        Ok(v) => render_entry(&v),
        Err(_) => String::from_utf8_lossy(bytes).trim_end().to_string(),
    }
}

#[cfg(test)]
#[path = "tests/value_tests.rs"]
mod tests;
