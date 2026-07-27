//! The probe evaluation sequence, owned once (COOK-359).
//!
//! Evaluating a probe is the same seven steps wherever it happens: resolve the
//! declared inputs, compute the fingerprint, decide whether the probe has a
//! cache key at all, look the key up, run `produce` on a miss, publish the
//! result, and materialise the canonical local copy. Only step five differs
//! between phases, and only in WHICH Lua VM runs the source: the register VM
//! for an `ingredients <probe>` pre-pass, a worker VM for a sealed consumer.
//! That one difference is why the sequence was written twice; [`ProduceRunner`]
//! makes it a parameter so it stops being a reason.
//!
//! What this module owns and what it does not:
//!
//!   * `cook-contracts` owns what a probe IS — [`ProbeUnit`], its declared
//!     inputs, and the pure rules for rendering and parsing a probe value. It
//!     is forbidden stateful std access by its own layout test, so it can
//!     describe a value but never fetch, store, or run one.
//!   * This module owns what EVALUATING one does: filesystem, cache backend,
//!     and the ordering between them.
//!   * The caller owns scheduling, the VM, event emission, and diagnostics. It
//!     is handed [`Evaluated`] and decides what to say about it.
//!
//! Producer-kind interception lives here on purpose. COOK-353 was a `files { }`
//! probe whose reserved `@files-manifest` sentinel the executor intercepted and
//! the pre-pass did not, so the sentinel reached the register VM and died as a
//! Lua syntax error on a bare `@`. A new producer kind can now only be taught
//! to the sequence, never to one phase.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cook_contracts::ProbeUnit;

/// The one genuinely phase-specific step: run a `produce` source on this
/// phase's Lua VM and return its canonical value bytes.
///
/// Implementors are the register VM (sandboxed, single-threaded, in-process)
/// and the worker VM (execute-phase policy). The sequence around the call is
/// identical, which is the whole point of the seam.
pub trait ProduceRunner {
    fn run(&self, key: &str, source: &str) -> Result<Vec<u8>, String>;
}

/// A failure with enough context for either caller to render its own
/// diagnostic. The sequence never prints; it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// A declared input could not be resolved — most often an upstream
    /// `requires` whose fingerprint is not yet known.
    ResolveInputs { key: String, message: String },
    /// The `produce` source failed on the caller's VM.
    Produce { key: String, message: String },
}

impl ProbeError {
    pub fn key(&self) -> &str {
        match self {
            ProbeError::ResolveInputs { key, .. } | ProbeError::Produce { key, .. } => key,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ProbeError::ResolveInputs { message, .. } | ProbeError::Produce { message, .. } => {
                message
            }
        }
    }
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "probe '{}': {}", self.key(), self.message())
    }
}

/// Cache access for one invocation. `None` on [`EvalCtx::cache`] means no
/// backend is wired, and the sequence degrades to always-produce.
///
/// Note for callers: "no backend" must mean genuinely no backend. COOK-359's
/// root cause was a caller that passed `None` because nobody had wired the
/// context, which silently converted every GET into a miss and made an
/// `ingredients <probe>` driver re-produce on every invocation for the life of
/// the feature.
pub struct CacheAccess<'a> {
    pub backend: &'a dyn cook_fingerprint::backend::CacheBackend,
    /// Root under which `.cook/probes/` is written.
    pub project_root: &'a Path,
    /// COOK-168: false suppresses every shared-store upload for this
    /// invocation. Fetch is unaffected.
    pub publish_enabled: bool,
}

/// Everything the sequence needs that is neither the probe nor the VM.
pub struct EvalCtx<'a> {
    /// Base for resolving the probe's declared `files` inputs.
    pub working_dir: &'a Path,
    pub cache: Option<CacheAccess<'a>>,
}

impl EvalCtx<'_> {
    /// Where the canonical local copy goes. Falls back to `working_dir` when
    /// no backend is wired, matching the behaviour of a workspace that has no
    /// resolved project root.
    fn probes_dir(&self) -> PathBuf {
        let root = match &self.cache {
            Some(c) => c.project_root,
            None => self.working_dir,
        };
        root.join(".cook").join("probes")
    }
}

/// The outcome of one probe evaluation.
#[derive(Debug, Clone)]
pub struct Evaluated {
    /// Canonical value bytes. Byte-identical across phases for the same probe.
    pub bytes: Vec<u8>,
    pub fingerprint: [u8; 32],
    /// CS-0178: this probe declares nothing (or reaches something that
    /// doesn't), so it has no cache key. Callers propagate this into the set
    /// they pass as `keyless_upstreams` for probes that `require` it.
    pub keyless: bool,
    /// True when the value came from the cache and `produce` never ran.
    pub cache_hit: bool,
    /// CS-0157: where each declared tool resolves RIGHT NOW. Location
    /// metadata, deliberately outside the fingerprint and the canonical value,
    /// so it can never go stale inside a cached value.
    pub tool_paths: BTreeMap<String, String>,
    /// Non-fatal conditions worth surfacing. Returned rather than printed:
    /// the two callers log through different channels (`eprintln!` at register
    /// phase, `tracing` in the executor), and that divergence was itself one
    /// of the differences between the two copies.
    pub warnings: Vec<String>,
}

/// Evaluate one probe.
///
/// `upstream_fps` must already hold a fingerprint for every key in
/// `probe.inputs.requires`; `keyless_upstreams` must hold the keys among them
/// that are themselves keyless. Both are the caller's to maintain, because
/// ordering `requires` is a scheduling concern and scheduling is exactly what
/// the two phases do differently for reasons that are not incidental.
pub fn evaluate(
    probe: &ProbeUnit,
    ctx: &EvalCtx<'_>,
    runner: &dyn ProduceRunner,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fps: &BTreeMap<String, [u8; 32]>,
    keyless_upstreams: &BTreeSet<String>,
) -> Result<Evaluated, ProbeError> {
    let key = probe.key.as_str();
    let mut warnings = Vec::new();

    // 1. Resolve declared inputs (env / tools / files / upstream fingerprints).
    let inputs =
        cook_fingerprint::probe::resolve_probe_inputs(probe, ctx.working_dir, env_lookup, upstream_fps)
            .map_err(|message| ProbeError::ResolveInputs { key: key.to_string(), message })?;

    // 2. Fingerprint. §22.5.4 sections 1-3 are always present; 4-7 are empty
    //    unless declared.
    let fingerprint = cook_fingerprint::compute_probe_fingerprint(&inputs);

    // 3. CS-0178 keylessness. A probe declaring nothing has a fingerprint built
    //    from the marker, the key, and the produce source alone — constant for
    //    the life of the store — so consulting the cache would answer it once
    //    and never observe again. Keylessness propagates along `requires`,
    //    because folding section 7 over a constant upstream fingerprint would
    //    serve this probe across the very change it exists to notice.
    let declares_nothing = probe.inputs.env.is_empty()
        && probe.inputs.tools.is_empty()
        && probe.inputs.files.is_empty()
        && probe.inputs.requires.is_empty();
    let reaches_keyless = probe.inputs.requires.iter().any(|k| keyless_upstreams.contains(k));
    let keyless = declares_nothing || reaches_keyless;

    // 4. CS-0157 tool locations. Resolved before the hit/miss fork so both
    //    paths carry them: a cache hit still needs to tell a consumer where
    //    the tool is NOW, which is precisely what a cached value must not say.
    let mut tool_paths = BTreeMap::new();
    for (name, _identity) in &inputs.tools {
        if let Some(path) = cook_fingerprint::resolve_tool_path(name) {
            tool_paths.insert(name.clone(), path);
        }
    }

    // 5. Cache GET, unless there is no key to look up.
    let mut cache_hit = false;
    let cached: Option<Vec<u8>> = match (&ctx.cache, keyless) {
        (Some(access), false) => match cook_cache::backend::get_bytes(access.backend, &fingerprint) {
            Ok(Some(bytes)) if cook_contracts::probe_value::decode_json(&bytes).is_ok() => {
                Some(bytes)
            }
            // CS-0102 stale-artifact defence, second layer behind the V2
            // fingerprint marker. Evict rather than merely ignoring: a key that
            // addresses unparseable bytes stays addressable to every other
            // reader of a SHARED store until something removes it, and the put
            // below then self-heals it. (The executor previously fell through
            // silently, leaving the poisoned key in place.)
            Ok(Some(_)) => {
                warnings.push(format!(
                    "probe '{key}': cached bytes are not probe-value JSON \
                     (pre-CS-0102 artifact?); treating as miss"
                ));
                let _ = access.backend.delete(&fingerprint);
                None
            }
            Ok(None) => None,
            Err(e) => {
                warnings.push(format!(
                    "probe '{key}': cache backend error on get ({e}); treating as miss"
                ));
                None
            }
        },
        _ => None,
    };

    // 6. On a miss, produce. A `files { }` probe never reaches a VM: its
    //    produce string is the reserved `@files-manifest` sentinel, which is
    //    deliberately not valid Lua so that a path which tried to run it would
    //    fail loudly. The value is synthesised from the same path→hash pairs
    //    the fingerprint's FILES section just folded, so every phase agrees on
    //    it byte for byte.
    let bytes = match cached {
        Some(bytes) => {
            cache_hit = true;
            bytes
        }
        None if is_files_manifest(probe) => {
            cook_contracts::probe_value::encode_files_manifest(&inputs.files)
        }
        None => runner
            .run(key, &probe.produce_source)
            .map_err(|message| ProbeError::Produce { key: key.to_string(), message })?,
    };

    // 7. Publish, then materialise the canonical local copy. A keyless probe
    //    publishes nothing: skipping only the GET would leave a stable
    //    fingerprint addressing a stored value that another reader — a
    //    verifier, another machine on the shared store, a future run under a
    //    changed rule — could still be served.
    if let Some(access) = &ctx.cache {
        if !cache_hit && !keyless && access.publish_enabled {
            let mut meta = probe_artifact_meta(key, bytes.len());
            // Non-fatal: the value is already in hand for this invocation, so
            // a publish failure costs later runs a hit and costs this one
            // nothing.
            if let Err(e) =
                cook_cache::backend::put_bytes(access.backend, &fingerprint, &bytes, &mut meta)
            {
                warnings.push(format!(
                    "probe '{key}': cache backend put failed ({e}); continuing without caching"
                ));
            }
        }
    }

    // CS-0102: the canonical local copy at `.cook/probes/<key>.json`, holding
    // the same bytes as the per-run store and the CAS artifact. Non-fatal.
    let probes_dir = ctx.probes_dir();
    if let Err(e) = crate::store::materialize_value(&probes_dir, key, &bytes) {
        warnings.push(format!(
            "probe '{key}': failed to write {}: {e}",
            probes_dir.display()
        ));
    }

    Ok(Evaluated { bytes, fingerprint, keyless, cache_hit, tool_paths, warnings })
}

/// CS-0148: a `files { }` producer is intercepted, never run.
fn is_files_manifest(probe: &ProbeUnit) -> bool {
    probe.produce_source == cook_contracts::probe_value::FILES_MANIFEST_PRODUCE
}

/// Cache metadata for a stored probe value. Identical in both phases; it was
/// duplicated field-for-field before.
fn probe_artifact_meta(key: &str, size: usize) -> cook_fingerprint::ArtifactMeta {
    cook_fingerprint::ArtifactMeta {
        recipe_namespace: format!("probe:{key}"),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: cook_fingerprint::CACHE_VERSION,
        size_bytes: size as u64,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: format!("probe:{key}"),
        content_hash: cook_fingerprint::ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: cook_fingerprint::ArtifactMeta::default_mode(),
        target: None,
    }
    .as_probe_value()
}

#[cfg(test)]
mod tests;
