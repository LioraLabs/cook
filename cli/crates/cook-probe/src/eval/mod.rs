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
/// This is the seam for a caller that produces SYNCHRONOUSLY — the register
/// pre-pass, which runs the source inline on the register VM. The executor
/// cannot use it: it hands the work to a thread pool and returns to the
/// scheduler, so a blocking call here would serialise probe production behind
/// the scheduler thread. That caller drives [`lookup`] and [`record`] directly
/// and keeps its own dispatch in between; [`evaluate`] is the two of them plus
/// a `ProduceRunner`, which is exactly what a synchronous caller needs.
pub trait ProduceRunner {
    fn run(&self, key: &str, source: &str) -> Result<Produced, String>;
}

/// What running a `produce` body yielded: the canonical value bytes, and the
/// module files the body loaded on the way there (CS-0204).
///
/// The second half is not decoration. §22.5.3's fingerprint folds seven
/// DECLARED sections and no module source, so before CS-0204 a `produce` body
/// that called into a module stayed addressable at the same fingerprint after
/// the module changed, and the consumer was served the old answer. The set can
/// only be known by running the body, which is why it comes back with the
/// bytes rather than being computed alongside the fingerprint.
#[derive(Debug, Clone, Default)]
pub struct Produced {
    pub bytes: Vec<u8>,
    /// Working-directory-relative where possible; hashed by joining onto the
    /// reader's own working directory, so the same list means the same thing
    /// on another machine.
    pub module_paths: Vec<String>,
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
    pub backend: &'a dyn cook_cache::backend::CacheBackend,
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
        cook_contracts::layout::probes_dir(root)
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

/// Where a probe's bytes came from. Decides whether they get published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    /// Served by the cache. Already stored, so [`record`] must not re-publish.
    Cache,
    /// Newly produced — by a VM, or synthesised for a producer kind that needs
    /// none. Published subject to keylessness and `publish_enabled`.
    Produced,
}

/// Everything decided before a value exists: the fingerprint, whether there is
/// a key at all, where the declared tools resolve, and either the bytes (when
/// no VM is needed) or nothing (when the caller must produce).
pub struct Lookup {
    /// This probe's identity as best known right now: the DECLARED fingerprint
    /// on a miss, and on a cache hit the FULL one that the winning candidate
    /// module set composed (CS-0204). Callers propagate it to downstream
    /// `requires` and hand it back to [`record`], so a module edit rekeys the
    /// whole downstream chain rather than just the probe that loaded it.
    pub fingerprint: [u8; 32],
    pub keyless: bool,
    pub tool_paths: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    /// `Some` when the value is already determined without running a VM:
    /// either the cache served it, or the producer kind is synthesised
    /// (CS-0148 `files { }`). `None` means the caller must produce.
    pub resolved: Option<(Vec<u8>, ValueSource)>,
}

/// Steps 1-5: resolve inputs, fingerprint, decide keylessness, resolve tool
/// locations, consult the cache, and intercept producer kinds that need no VM.
///
/// `upstream_fps` must already hold a fingerprint for every key in
/// `probe.inputs.requires`; `keyless_upstreams` must hold the keys among them
/// that are themselves keyless. Both are the caller's to maintain, because
/// ordering `requires` is a scheduling concern and scheduling is exactly what
/// the two phases do differently for reasons that are not incidental.
pub fn lookup(
    probe: &ProbeUnit,
    ctx: &EvalCtx<'_>,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fps: &BTreeMap<String, [u8; 32]>,
    keyless_upstreams: &BTreeSet<String>,
) -> Result<Lookup, ProbeError> {
    let key = probe.key.as_str();
    let mut warnings = Vec::new();

    // 1. Resolve declared inputs (env / tools / files / upstream fingerprints).
    let inputs =
        cook_cache::probe::resolve_probe_inputs(probe, ctx.working_dir, env_lookup, upstream_fps)
            .map_err(|message| ProbeError::ResolveInputs { key: key.to_string(), message })?;

    // 2. Fingerprint. §22.5.4 sections 1-3 are always present; 4-7 are empty
    //    unless declared.
    let fingerprint = cook_cache::compute_probe_fingerprint(&inputs);

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
        if let Some(path) = cook_cache::resolve_tool_path(name) {
            tool_paths.insert(name.clone(), path);
        }
    }

    // 5. Cache GET, unless there is no key to look up.
    //
    // CS-0204 makes this two-level. The declared fingerprint identifies the
    // probe; the value is stored under the fingerprint that ALSO folds the
    // content of the modules the body loaded, and which modules those are is
    // knowable only by having run it. So the recorded candidate sets are read
    // from a manifest keyed by the declared fingerprint, re-hashed against
    // THIS machine's tree, and the composed full fingerprints probed
    // newest-first. A set that no longer describes this tree composes a
    // fingerprint nothing is stored under: a safe miss, never a wrong hit.
    //
    // A probe that has never loaded a module has no manifest, the single empty
    // candidate composes the declared fingerprint unchanged, and this is
    // byte-for-byte the pre-CS-0204 lookup.
    let declared_fingerprint = fingerprint;
    let mut fingerprint = declared_fingerprint;
    let mut cached: Option<Vec<u8>> = None;
    if let (Some(access), false) = (&ctx.cache, keyless) {
        for candidate in module_candidates(access.backend, &declared_fingerprint) {
            let full = fold_candidate(&declared_fingerprint, ctx.working_dir, &candidate);
            match cook_cache::backend::get_bytes(access.backend, &full) {
                Ok(Some(bytes)) if cook_contracts::probe_value::decode_json(&bytes).is_ok() => {
                    // Only a DECODABLE value settles the identity. A candidate
                    // that hit and then failed to parse is evicted below and
                    // leaves the identity where it was, so the miss that
                    // follows re-folds from the declared fingerprint rather
                    // than from an already-folded one.
                    fingerprint = full;
                    cached = Some(bytes);
                    break;
                }
                // CS-0102 stale-artifact defence, second layer behind the V2
                // fingerprint marker. Evict rather than merely ignoring: a key
                // that addresses unparseable bytes stays addressable to every
                // other reader of a SHARED store until something removes it,
                // and the put below then self-heals it.
                Ok(Some(_)) => {
                    warnings.push(format!(
                        "probe '{key}': cached bytes are not probe-value JSON \
                         (pre-CS-0102 artifact?); treating as miss"
                    ));
                    let _ = access.backend.delete(&full);
                }
                Ok(None) => {}
                Err(e) => {
                    warnings.push(format!(
                        "probe '{key}': cache backend error on get ({e}); treating as miss"
                    ));
                    break;
                }
            }
        }
    }

    // 6. Decide whether a VM is needed at all. A `files { }` probe never
    //    reaches one: its produce string is the reserved `@files-manifest`
    //    sentinel, deliberately not valid Lua so that a path which tried to run
    //    it would fail loudly. The value is synthesised from the same path→hash
    //    pairs the fingerprint's FILES section just folded, so every phase
    //    agrees on it byte for byte.
    let resolved = match cached {
        Some(bytes) => Some((bytes, ValueSource::Cache)),
        None if is_files_manifest(probe) => Some((
            cook_contracts::probe_value::encode_files_manifest(&inputs.files),
            ValueSource::Produced,
        )),
        None => None,
    };

    Ok(Lookup { fingerprint, keyless, tool_paths, warnings, resolved })
}

/// What [`record`] did.
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    /// CS-0204: the fingerprint the value is stored under — the declared one
    /// folded with the module content the body ran against. This is the
    /// probe's true identity, and it is what callers propagate to downstream
    /// `requires` so a module edit rekeys the whole chain.
    pub fingerprint: [u8; 32],
    pub warnings: Vec<String>,
    /// True when bytes were actually written to the shared store. Reported
    /// rather than left for the caller to re-derive from source/keylessness/
    /// `publish_enabled` — re-deriving it is how a rule ends up implemented
    /// twice, which is the defect this crate exists to retire.
    pub published: bool,
}

/// Steps 6-7: publish the value if it is new, then materialise the canonical
/// local copy. Returns any non-fatal conditions; never prints.
///
/// Split from [`lookup`] because the executor produces asynchronously: it looks
/// up at dispatch, hands a miss to a worker, returns to the scheduler, and
/// records whatever the worker eventually sends back.
/// Takes the fingerprint and keylessness rather than a whole [`Lookup`], so a
/// caller whose value arrived from a worker long after the lookup — the
/// executor — can record it without reconstructing one.
pub fn record(
    key: &str,
    ctx: &EvalCtx<'_>,
    fingerprint: &[u8; 32],
    keyless: bool,
    bytes: &[u8],
    source: ValueSource,
    // CS-0204: the modules the `produce` body loaded. Empty on a cache hit
    // (nothing ran) and for a body that loaded nothing, in which case every
    // decision below is the pre-CS-0204 one.
    module_paths: &[String],
) -> Recorded {
    let mut warnings = Vec::new();
    let mut published = false;

    // CS-0204: a produced value is stored under the fingerprint that folds the
    // module content it ran against, not under the declared one. `fingerprint`
    // IS the declared one on this path, because a miss is what brought us here.
    // On a cache hit nothing is published, so the caller's (already full)
    // fingerprint passes through untouched.
    let stored_fingerprint = if source == ValueSource::Produced {
        fold_candidate(fingerprint, ctx.working_dir, module_paths)
    } else {
        *fingerprint
    };

    // A keyless probe publishes nothing. Skipping only the GET would leave a
    // stable fingerprint addressing a stored value that another reader — a
    // verifier, another machine on the shared store, a future run under a
    // changed rule — could still be served.
    if let Some(access) = &ctx.cache {
        if source == ValueSource::Produced && !keyless && access.publish_enabled {
            let mut meta = probe_artifact_meta(key, bytes.len());
            // Non-fatal: the value is already in hand for this invocation, so a
            // publish failure costs later runs a hit and costs this one nothing.
            match cook_cache::backend::put_bytes(
                access.backend,
                &stored_fingerprint,
                bytes,
                &mut meta,
            ) {
                Ok(()) => published = true,
                Err(e) => warnings.push(format!(
                    "probe '{key}': cache backend put failed ({e}); continuing without caching"
                )),
            }
            // The bridge a cold reader crosses: without this, the value just
            // published sits at a fingerprint no one else can compute.
            if published && !module_paths.is_empty() {
                warnings.extend(publish_module_manifest(access, fingerprint, module_paths));
            }
        }
    }

    // CS-0102: the canonical local copy at `.cook/probes/<key>.json`, holding
    // the same bytes as the per-run store and the CAS artifact. Non-fatal.
    let probes_dir = ctx.probes_dir();
    if let Err(e) = crate::store::materialize_value(&probes_dir, key, bytes) {
        warnings.push(format!(
            "probe '{key}': failed to write {}: {e}",
            probes_dir.display()
        ));
    }

    Recorded { fingerprint: stored_fingerprint, warnings, published }
}

/// The candidate module-path sets to try, newest first.
///
/// Always at least one: the empty set, which folds to the declared
/// fingerprint. That is the whole of the pre-CS-0204 behaviour for a probe
/// that loads nothing, and a correct first guess for one whose manifest has
/// been evicted (it misses, and the re-run republishes).
fn module_candidates(
    backend: &dyn cook_cache::backend::CacheBackend,
    declared: &[u8; 32],
) -> Vec<Vec<String>> {
    cook_cache::path_set_candidates(read_module_manifest(backend, declared))
}

/// The recorded path sets under a probe's declared fingerprint. Absent,
/// unreadable or malformed all decode to none, which the caller reads as
/// "nothing to reconstruct from" and turns into a safe miss.
fn read_module_manifest(
    backend: &dyn cook_cache::backend::CacheBackend,
    declared: &[u8; 32],
) -> Vec<Vec<String>> {
    let manifest_key = cook_contracts::context::probe_module_manifest_key(declared);
    cook_cache::backend::get_bytes(backend, &manifest_key)
        .ok()
        .flatten()
        .map(|b| cook_cache::decode_path_sets(&b))
        .unwrap_or_default()
}

/// Compose the full fingerprint for one candidate set by hashing its paths
/// against THIS working directory. The hashing is `cook_cache`'s, the same
/// function §22.5.3's FILES section folds with; the composition is the
/// Standard's, in `cook_contracts`.
///
/// Deliberately unmemoised, unlike the tool-binary hashes beside it. A tool is
/// a ~60MB binary hashed once per probe NODE across a whole workspace; a module
/// is a few KB of Lua, hashed once per candidate set, and `MODULE_SET_CAP`
/// bounds the candidates at eight — with the first one hitting in the settled
/// case. A memo here would buy nothing and would have to be invalidated the
/// moment a run started writing modules.
fn fold_candidate(declared: &[u8; 32], working_dir: &Path, paths: &[String]) -> [u8; 32] {
    if paths.is_empty() {
        return *declared;
    }
    let hashed: Vec<(String, [u8; 32])> = paths
        .iter()
        .map(|p| (p.clone(), cook_cache::hash_file_sha256(&working_dir.join(p))))
        .collect();
    cook_contracts::context::fold_module_sources(declared, &hashed)
}

/// Record this run's module set under the DECLARED fingerprint so a cold
/// reader can reconstruct the full one. Returns warnings; never fatal, because
/// the value itself is already stored and a lost manifest costs a re-run.
fn publish_module_manifest(
    access: &CacheAccess<'_>,
    declared: &[u8; 32],
    observed: &[String],
) -> Vec<String> {
    let manifest_key = cook_contracts::context::probe_module_manifest_key(declared);
    let existing = read_module_manifest(access.backend, declared);
    // Merge and wire form are the shared law, so this store and the step store
    // cannot drift on what the manifest means or on how it is written down.
    let sets = cook_cache::merge_path_set(&existing, observed);
    let json = cook_cache::encode_path_sets(&sets);
    let mut meta = probe_artifact_meta(cook_cache::MODULE_INPUT_SETS_PATH, json.len());
    meta.kind = Some("module_input_sets".to_string());
    match cook_cache::backend::put_bytes(access.backend, &manifest_key, &json, &mut meta) {
        Ok(()) => Vec::new(),
        Err(e) => vec![format!(
            "probe module manifest: put failed ({e}); the value just published \
             will not be reachable from a cold lookup until the next run"
        )],
    }
}

/// [`lookup`] + produce + [`record`], for a caller that produces synchronously.
pub fn evaluate(
    probe: &ProbeUnit,
    ctx: &EvalCtx<'_>,
    runner: &dyn ProduceRunner,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fps: &BTreeMap<String, [u8; 32]>,
    keyless_upstreams: &BTreeSet<String>,
) -> Result<Evaluated, ProbeError> {
    let key = probe.key.as_str();
    let mut found = lookup(probe, ctx, env_lookup, upstream_fps, keyless_upstreams)?;

    let (bytes, module_paths, source) = match found.resolved.take() {
        Some((bytes, source)) => (bytes, Vec::new(), source),
        None => {
            let produced = runner
                .run(key, &probe.produce_source)
                .map_err(|message| ProbeError::Produce { key: key.to_string(), message })?;
            (produced.bytes, produced.module_paths, ValueSource::Produced)
        }
    };

    let mut warnings = std::mem::take(&mut found.warnings);
    let recorded = record(
        key,
        ctx,
        &found.fingerprint,
        found.keyless,
        &bytes,
        source,
        &module_paths,
    );
    warnings.extend(recorded.warnings);

    Ok(Evaluated {
        bytes,
        fingerprint: recorded.fingerprint,
        keyless: found.keyless,
        cache_hit: source == ValueSource::Cache,
        tool_paths: found.tool_paths,
        warnings,
    })
}

/// CS-0148: a `files { }` producer is intercepted, never run.
fn is_files_manifest(probe: &ProbeUnit) -> bool {
    probe.produce_source == cook_contracts::probe_value::FILES_MANIFEST_PRODUCE
}

/// Cache metadata for a stored probe value. Identical in both phases; it was
/// duplicated field-for-field before.
fn probe_artifact_meta(key: &str, size: usize) -> cook_cache::ArtifactMeta {
    cook_cache::ArtifactMeta {
        recipe_namespace: format!("probe:{key}"),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: cook_cache::CACHE_VERSION,
        size_bytes: size as u64,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: format!("probe:{key}"),
        content_hash: cook_cache::ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: cook_cache::ArtifactMeta::default_mode(),
        target: None,
    }
    .as_probe_value()
}

#[cfg(test)]
mod tests;
