//! The probe evaluation sequence, owned once (COOK-359).
//!
//! Evaluating a probe is the same steps wherever it happens: resolve the
//! declared inputs, compute the fingerprint, decide whether the probe has a
//! cache key at all, decide whether a value is already resolved without
//! running a VM, run `produce` otherwise, and materialise the canonical local
//! copy. Only the produce step differs between phases, and only in WHICH Lua
//! VM runs the source: the register VM for a `gather <probe>` pre-pass, a
//! worker VM for a sealed consumer. That one difference is why the sequence
//! was written twice; [`ProduceRunner`] makes it a parameter so it stops
//! being a reason.
//!
//! CS-0243: a reached probe always observes. There is no probe-value cache —
//! no GET, no PUT, no publish, no stored artifact. The only two ways a value
//! is resolved without running a VM are not caching: a top-level
//! `files`/`tools` declaration's value is synthesised fresh from the current
//! tree, and a key this same invocation's register pre-pass already resolved
//! is served from that pre-pass's own production (CS-0242's cross-phase
//! single-flight). Neither reads a previous invocation's answer.
//!
//! What this module owns and what it does not:
//!
//!   * `cook-contracts` owns what a probe IS — [`ProbeUnit`], its declared
//!     inputs, and the pure rules for rendering and parsing a probe value. It
//!     is forbidden stateful std access by its own layout test, so it can
//!     describe a value but never fetch, store, or run one.
//!   * This module owns what EVALUATING one does: filesystem and the ordering
//!     of the steps above.
//!   * The caller owns scheduling, the VM, event emission, and diagnostics. It
//!     is handed [`Evaluated`] and decides what to say about it.
//!
//! Declaration-value interception lives here on purpose. COOK-353 was a top-level `files`
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

/// Everything the sequence needs that is neither the probe nor the VM.
pub struct EvalCtx<'a> {
    /// Base for resolving the probe's declared `files` inputs.
    pub working_dir: &'a Path,
    /// Root under which `.cook/probes/` is written. `None` falls back to
    /// `working_dir`, matching the behaviour of a workspace that has no
    /// resolved project root.
    pub project_root: Option<&'a Path>,
}

impl EvalCtx<'_> {
    /// Where the canonical local copy goes.
    fn probes_dir(&self) -> PathBuf {
        let root = self.project_root.unwrap_or(self.working_dir);
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

/// Where a probe's bytes came from. CS-0243: neither variant is ever
/// published — there is no store to publish to. [`record`] keeps the
/// distinction only to compute [`Recorded::fingerprint`] (CS-0204's module
/// folding applies to a freshly produced value, never to one already
/// resolved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    /// Newly produced — by a VM, or synthesised for a producer kind that needs
    /// none.
    Produced,
    /// COOK-526: served from this SAME invocation's register pre-pass, which
    /// already ran `produce` and already materialised the value at
    /// `.cook/probes/<key>.json` — `record`'s canonical-copy write is
    /// unconditional. Deliberately not a cache tier (CS-0243): the bytes come
    /// from THIS invocation's own production, never an earlier one's.
    Prepass,
}

/// Everything decided before a value exists: the fingerprint, whether there is
/// a key at all, where the declared tools resolve, and either the bytes (when
/// no VM is needed) or nothing (when the caller must produce).
pub struct Lookup {
    /// This probe's identity as best known right now: always the DECLARED
    /// fingerprint (§22.5.4). CS-0243: there is no cache to consult, so
    /// nothing here ever composes the FULL fingerprint — [`record`] does
    /// that, folding in the loaded module set (CS-0204's `fold_candidate`),
    /// after a value is actually produced. Callers hand this fingerprint to
    /// [`record`] and propagate ITS result to downstream `requires`.
    pub fingerprint: [u8; 32],
    pub keyless: bool,
    pub tool_paths: BTreeMap<String, String>,
    /// `Some` when the value is already determined without running a VM.
    /// CS-0243: neither way below is a cache. Two ways, in the order
    /// [`lookup`] tries them: a top-level `files`/`tools` declaration's value
    /// is synthesised fresh from the current tree; or this same invocation's
    /// register pre-pass already produced it (COOK-526, `prepass_resolved`).
    /// `None` means the caller must produce.
    pub resolved: Option<(Vec<u8>, ValueSource)>,
}

/// Resolve inputs, fingerprint, decide keylessness, resolve tool locations,
/// intercept producer kinds that need no VM, and serve a value this
/// invocation's register pre-pass already produced. CS-0243: there is no
/// cache to consult — a probe's value is never served from a store that
/// outlives the invocation.
///
/// `upstream_fps` must already hold a fingerprint for every key in
/// `probe.inputs.requires`; `keyless_upstreams` must hold the keys among them
/// that are themselves keyless. Both are the caller's to maintain, because
/// ordering `requires` is a scheduling concern and scheduling is exactly what
/// the two phases do differently for reasons that are not incidental.
///
/// `prepass_resolved` (COOK-526) says whether THIS invocation's register
/// pre-pass already resolved this exact key. Only the execute phase may pass
/// `true`: the register phase is the pre-pass, and a key consulting the
/// channel it is in the middle of filling would find itself already resolved
/// and serve its own previous run's file. [`evaluate`], the register phase's
/// entry point, passes `false` for that reason.
pub fn lookup(
    probe: &ProbeUnit,
    ctx: &EvalCtx<'_>,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fps: &BTreeMap<String, [u8; 32]>,
    keyless_upstreams: &BTreeSet<String>,
    prepass_resolved: bool,
) -> Result<Lookup, ProbeError> {
    let key = probe.key.as_str();

    // 1. Resolve declared inputs (env / tools / files / upstream fingerprints).
    let inputs =
        cook_cache::probe::resolve_probe_inputs(probe, ctx.working_dir, env_lookup, upstream_fps)
            .map_err(|message| ProbeError::ResolveInputs {
            key: key.to_string(),
            message,
        })?;

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

    // 4b. CS-0214: a top-level `tools` declaration fails, by name, when it
    //     cannot obtain a declared tool's identity. The rule used to live
    //     inside the emitted produce body, which put it behind the cache: a
    //     stored value could serve a probe whose tool had since been
    //     uninstalled. It is checked here, ahead of the GET, so it holds on hit
    //     and miss alike.
    //
    //     Two ways to have no identity, and the second is the one that bites.
    //     A name that does not resolve is the obvious case. A name that
    //     RESOLVES but whose bytes cannot be read is the dangerous one:
    //     `which` selects on `X_OK`, not `R_OK`, so an execute-only binary
    //     gets past it, and `hash_file_sha256` answers the all-zero digest for
    //     anything it cannot read. Rendering that into the value would put the
    //     same 64 zeros in every such value, so two hosts each failing to read
    //     a DIFFERENT toolchain would compose identical bytes and one could be
    //     served the other's sealed artifact. The deleted Lua producer could
    //     not reach this state — `sha256sum` exited non-zero and failed the
    //     probe — and neither may this one.
    //
    //     Only the synthesised producer is subject to either check. A
    //     hand-written body that happens to declare `inputs.tools` keeps
    //     folding an absent tool as the all-zero digest, which is what §22.5.4
    //     says it does; that probe's value is the author's to compute.
    if is_tools_identity(probe) {
        for (name, digest) in &inputs.tools {
            let Some(path) = tool_paths.get(name) else {
                return Err(ProbeError::Produce {
                    key: key.to_string(),
                    message: format!("tools declaration: '{name}' not found on PATH"),
                });
            };
            if digest == &[0u8; 32] {
                return Err(ProbeError::Produce {
                    key: key.to_string(),
                    message: format!(
                        "tools declaration: '{name}' resolved to {path} but its bytes \
                         could not be read, so it has no identity to record"
                    ),
                });
            }
        }
    }

    // 4c. COOK-510: a top-level `files` declaration fails, by name, when a
    //     matched path EXISTS but its bytes cannot be read. Mirrors 4b above,
    //     for the same reason: `hash_file_sha256` answers the identical
    //     all-zero digest for "does not exist" and "exists but unreadable",
    //     and §{cat.probes.decl} deliberately folds the former as the
    //     placeholder `"<missing>"` — a glob matching nothing is ordinary.
    //     Folding the latter the same way is not: the placeholder is
    //     indistinguishable from a path that was never there, so it freezes
    //     the synthesised manifest — and every unit that seals it — at a
    //     value that can never again observe an edit to that file's content.
    //     Checked ahead of the GET so it holds on hit and miss alike, the
    //     same reason 4b is.
    //
    //     Only the synthesised producer is subject to this, mirroring CS-0214's
    //     own scope: a hand-written body that happens to declare `inputs.files`
    //     keeps folding an unreadable match as the all-zero digest, which is
    //     what §22.5.4 says it does; that probe's value is the author's to
    //     compute.
    if is_files_manifest(probe) {
        for (path, digest) in &inputs.files {
            if digest == &[0u8; 32] && ctx.working_dir.join(path).exists() {
                return Err(ProbeError::Produce {
                    key: key.to_string(),
                    message: format!(
                        "files declaration: '{path}' exists but its bytes could not \
                         be read, so it has no content to record"
                    ),
                });
            }
        }
    }

    // 5. Decide whether a VM is needed at all. CS-0243: there is no cache to
    //    consult here, so `fingerprint` is simply the declared one from step
    //    2. Two producer kinds never reach a VM: their produce strings are
    //    the reserved `@files-manifest` and `@tools-identity` sentinels,
    //    deliberately not valid Lua so that a path which tried to run one
    //    would fail loudly. Each value is synthesised from the same pairs the
    //    fingerprint's FILES / TOOLS section just folded, so trigger and
    //    value are one computation and every phase agrees on the bytes.
    //
    // 5b. COOK-526: nothing above found a value — but THIS invocation's
    //     register pre-pass may already have produced one for this exact key,
    //     before the execute phase existed (a `gather <probe>` fan-out, or a
    //     register-phase `cook.probes.get`). When it has, the bytes are
    //     already at `.cook/probes/<key>.json`: `record` below writes that
    //     file unconditionally. Reading them back is what re-running
    //     `produce` would answer (§22.5.8), so the body runs at most once per
    //     key per invocation across both phases.
    //
    //     Deliberately not a cache tier (CS-0243): the bytes read here are
    //     always this SAME invocation's own production, never an earlier
    //     invocation's. It is last so the synthesised producer kinds keep
    //     their existing meaning, and a missing file falls through to an
    //     ordinary produce rather than failing.
    let resolved = match () {
        _ if is_files_manifest(probe) => Some((
            cook_contracts::probe_value::encode_files_manifest(&inputs.files),
            ValueSource::Produced,
        )),
        _ if is_tools_identity(probe) => Some((
            cook_contracts::probe_value::encode_tools_identity(&inputs.tools),
            ValueSource::Produced,
        )),
        _ if prepass_resolved => crate::store::read_value(&ctx.probes_dir(), key)
            .map(|bytes| (bytes, ValueSource::Prepass)),
        _ => None,
    };

    Ok(Lookup {
        fingerprint,
        keyless,
        tool_paths,
        resolved,
    })
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
}

/// Materialise the canonical local copy. Returns any non-fatal conditions;
/// never prints.
///
/// CS-0243: this no longer publishes anywhere — there is no store. It
/// survives as a separate step from [`lookup`] because the executor produces
/// asynchronously: it looks up at dispatch, hands a miss to a worker, returns
/// to the scheduler, and records whatever the worker eventually sends back.
/// Takes the fingerprint and keylessness rather than a whole [`Lookup`], so a
/// caller whose value arrived from a worker long after the lookup — the
/// executor — can record it without reconstructing one.
///
/// `record`'s materialise call is load-bearing and unconditional: COOK-526's
/// cross-phase single-flight (`lookup`'s `prepass_resolved` path) reads this
/// exact file back within the same invocation.
pub fn record(
    key: &str,
    ctx: &EvalCtx<'_>,
    fingerprint: &[u8; 32],
    _keyless: bool,
    bytes: &[u8],
    source: ValueSource,
    // CS-0204: the modules the `produce` body loaded. Empty for a value that
    // was already resolved (nothing ran) and for a body that loaded nothing,
    // in which case every decision below is the pre-CS-0204 one.
    //
    // NOTE (COOK-528): this fingerprint machinery is production-dead once no
    // value is ever stored under it, but it is left computing and propagating
    // here deliberately — retiring it is the next ticket's work, not this
    // one's.
    module_paths: &[String],
) -> Recorded {
    let mut warnings = Vec::new();

    // CS-0204: a produced value's true identity folds the module content it
    // ran against, not just the declared fingerprint. `fingerprint` IS the
    // declared one on this path. A value that was already resolved
    // (`ValueSource::Prepass`) carries the caller's (already full)
    // fingerprint through untouched.
    let stored_fingerprint = if source == ValueSource::Produced {
        fold_candidate(fingerprint, ctx.working_dir, module_paths)
    } else {
        *fingerprint
    };

    // CS-0102/CS-0243: the canonical local copy at `.cook/probes/<key>.json`.
    // Unconditional — this is the sole place a probe's value lands anywhere
    // outside this invocation's own memory, a write-only forensic record
    // never read back as a source across invocations. Non-fatal.
    let probes_dir = ctx.probes_dir();
    if let Err(e) = crate::store::materialize_value(&probes_dir, key, bytes) {
        warnings.push(format!(
            "probe '{key}': failed to write {}: {e}",
            probes_dir.display()
        ));
    }

    Recorded {
        fingerprint: stored_fingerprint,
        warnings,
    }
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
        .map(|p| {
            (
                p.clone(),
                cook_cache::hash_file_sha256(&working_dir.join(p)),
            )
        })
        .collect();
    cook_contracts::context::fold_module_sources(declared, &hashed)
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
    // `false`: this IS the register pre-pass. See `lookup`'s `prepass_resolved`.
    let mut found = lookup(probe, ctx, env_lookup, upstream_fps, keyless_upstreams, false)?;

    let (bytes, module_paths, source) = match found.resolved.take() {
        Some((bytes, source)) => (bytes, Vec::new(), source),
        None => {
            let produced =
                runner
                    .run(key, &probe.produce_source)
                    .map_err(|message| ProbeError::Produce {
                        key: key.to_string(),
                        message,
                    })?;
            (produced.bytes, produced.module_paths, ValueSource::Produced)
        }
    };

    let recorded = record(
        key,
        ctx,
        &found.fingerprint,
        found.keyless,
        &bytes,
        source,
        &module_paths,
    );

    Ok(Evaluated {
        bytes,
        fingerprint: recorded.fingerprint,
        keyless: found.keyless,
        tool_paths: found.tool_paths,
        warnings: recorded.warnings,
    })
}

/// CS-0148: a top-level `files` declaration's value is synthesised, never run.
fn is_files_manifest(probe: &ProbeUnit) -> bool {
    probe.produce_source == cook_contracts::probe_value::FILES_MANIFEST_PRODUCE
}

/// CS-0214: a top-level `tools` declaration's value is synthesised, never run.
fn is_tools_identity(probe: &ProbeUnit) -> bool {
    probe.produce_source == cook_contracts::probe_value::TOOLS_IDENTITY_PRODUCE
}

#[cfg(test)]
mod tests;
