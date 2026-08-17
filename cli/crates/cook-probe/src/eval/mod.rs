//! The probe evaluation sequence, owned once (COOK-359).
//!
//! Evaluating a probe is the same steps wherever it happens: resolve the
//! declared `tools`/`files` sets, decide whether a value is already resolved
//! without running a VM, run `produce` otherwise, and materialise the
//! canonical local copy. Only the produce step differs between phases, and only in WHICH Lua
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

use std::collections::BTreeMap;
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

/// What running a `produce` body yielded: the canonical value bytes.
#[derive(Debug, Clone, Default)]
pub struct Produced {
    pub bytes: Vec<u8>,
}

/// A failure with enough context for either caller to render its own
/// diagnostic. The sequence never prints; it reports.
///
/// One shape, because the sequence has one way to fail: the `produce` source
/// failed on the caller's VM, or a top-level `tools`/`files` declaration could
/// not obtain what it must record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError {
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "probe '{}': {}", self.key, self.message)
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
    /// CS-0157: where each declared tool resolves RIGHT NOW. Location
    /// metadata, deliberately outside the canonical value, so it can never go
    /// stale inside a value another machine is served.
    pub tool_paths: BTreeMap<String, String>,
    /// Non-fatal conditions worth surfacing. Returned rather than printed:
    /// the two callers log through different channels (`eprintln!` at register
    /// phase, `tracing` in the executor), and that divergence was itself one
    /// of the differences between the two copies.
    pub warnings: Vec<String>,
}

/// Where a probe's bytes came from. CS-0243: neither variant is ever
/// published — there is no store to publish to. The distinction survives for
/// the caller's reporting: a synthesised value is work that ran, a pre-pass
/// value is work this invocation already did.
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

/// Everything decided before a value exists: where the declared tools
/// resolve, and either the bytes (when no VM is needed) or nothing (when the
/// caller must produce).
pub struct Lookup {
    pub tool_paths: BTreeMap<String, String>,
    /// `Some` when the value is already determined without running a VM.
    /// CS-0243: neither way below is a cache. Two ways, in the order
    /// [`lookup`] tries them: a top-level `files`/`tools` declaration's value
    /// is synthesised fresh from the current tree; or this same invocation's
    /// register pre-pass already produced it (COOK-526, `prepass_resolved`).
    /// `None` means the caller must produce.
    pub resolved: Option<(Vec<u8>, ValueSource)>,
}

/// Resolve the declared `tools`/`files` sets and tool locations, intercept
/// producer kinds that need no VM, and serve a value this invocation's
/// register pre-pass already produced. CS-0243: there is no cache to consult —
/// a probe's value is never served from a store that outlives the invocation.
///
/// CS-0244: `inputs.requires` is read nowhere here. It is a scheduling edge,
/// carried structurally — `unit_graph::plan` topologically orders the
/// synthesised probe keys by it, and `cook-register`'s `probe_api` wires it
/// onto the synthesised unit's `probes` field so the DAG holds it.
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
    prepass_resolved: bool,
) -> Result<Lookup, ProbeError> {
    let key = probe.key.as_str();

    // 1. Resolve the declared `tools` and `files` sets against the host.
    let inputs = cook_cache::resolve_probe_input_digests(probe, ctx.working_dir);

    // 2. CS-0157 tool locations. Resolved before the resolved/produce fork so
    //    both paths carry them: a value already in hand still needs to tell a
    //    consumer where the tool is NOW, which is precisely what the value
    //    itself must not say.
    let mut tool_paths = BTreeMap::new();
    for (name, _identity) in &inputs.tools {
        if let Some(path) = cook_cache::resolve_tool_path(name) {
            tool_paths.insert(name.clone(), path);
        }
    }

    // 3. CS-0214: a top-level `tools` declaration fails, by name, when it
    //     cannot obtain a declared tool's identity. The rule used to live
    //     inside the emitted produce body, which put it behind the then-extant
    //     cache: a stored value could serve a probe whose tool had since been
    //     uninstalled. It is checked here, ahead of the resolved/produce fork,
    //     so it holds however the value is obtained.
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
    //     hand-written body that happens to declare `inputs.tools` resolves an
    //     absent tool to the all-zero digest and nothing looks at it; that
    //     probe's value is the author's to compute.
    if is_tools_identity(probe) {
        for (name, digest) in &inputs.tools {
            let Some(path) = tool_paths.get(name) else {
                return Err(ProbeError {
                    key: key.to_string(),
                    message: format!("tools declaration: '{name}' not found on PATH"),
                });
            };
            if digest == &[0u8; 32] {
                return Err(ProbeError {
                    key: key.to_string(),
                    message: format!(
                        "tools declaration: '{name}' resolved to {path} but its bytes \
                         could not be read, so it has no identity to record"
                    ),
                });
            }
        }
    }

    // 4. COOK-510: a top-level `files` declaration fails, by name, when a
    //     matched path EXISTS but its bytes cannot be read. Mirrors 3 above,
    //     for the same reason: `hash_file_sha256` answers the identical
    //     all-zero digest for "does not exist" and "exists but unreadable",
    //     and §{cat.probes.decl} deliberately folds the former as the
    //     placeholder `"<missing>"` — a glob matching nothing is ordinary.
    //     Folding the latter the same way is not: the placeholder is
    //     indistinguishable from a path that was never there, so it freezes
    //     the synthesised manifest — and every unit that seals it — at a
    //     value that can never again observe an edit to that file's content.
    //     Checked ahead of the resolved/produce fork, the same reason 3 is.
    //
    //     Only the synthesised producer is subject to this, mirroring CS-0214's
    //     own scope: a hand-written body that happens to declare `inputs.files`
    //     resolves an unreadable match to the all-zero digest and nothing looks
    //     at it; that probe's value is the author's to compute.
    if is_files_manifest(probe) {
        for (path, digest) in &inputs.files {
            if digest == &[0u8; 32] && ctx.working_dir.join(path).exists() {
                return Err(ProbeError {
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
    //    consult here. Two producer kinds never reach a VM: their produce
    //    strings are the reserved `@files-manifest` and `@tools-identity`
    //    sentinels, deliberately not valid Lua so that a path which tried to
    //    run one would fail loudly. Each value is synthesised from the same
    //    pairs step 1 resolved and steps 3 and 4 just guarded, so the value
    //    and the guard are one computation and every phase agrees on the
    //    bytes.
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
        tool_paths,
        resolved,
    })
}

/// What [`record`] did.
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    pub warnings: Vec<String>,
}

/// Materialise the canonical local copy. Returns any non-fatal conditions;
/// never prints.
///
/// CS-0243: this no longer publishes anywhere — there is no store. It
/// survives as a separate step from [`lookup`] because the executor produces
/// asynchronously: it looks up at dispatch, hands a miss to a worker, returns
/// to the scheduler, and records whatever the worker eventually sends back.
/// Takes the key and the bytes rather than a whole [`Lookup`], so a caller
/// whose value arrived from a worker long after the lookup — the executor —
/// can record it without reconstructing one.
///
/// `record`'s materialise call is load-bearing and unconditional: COOK-526's
/// cross-phase single-flight (`lookup`'s `prepass_resolved` path) reads this
/// exact file back within the same invocation.
pub fn record(key: &str, ctx: &EvalCtx<'_>, bytes: &[u8]) -> Recorded {
    let mut warnings = Vec::new();

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

    Recorded { warnings }
}

/// [`lookup`] + produce + [`record`], for a caller that produces synchronously.
pub fn evaluate(
    probe: &ProbeUnit,
    ctx: &EvalCtx<'_>,
    runner: &dyn ProduceRunner,
) -> Result<Evaluated, ProbeError> {
    let key = probe.key.as_str();
    // `false`: this IS the register pre-pass. See `lookup`'s `prepass_resolved`.
    let mut found = lookup(probe, ctx, false)?;

    let bytes = match found.resolved.take() {
        Some((bytes, _source)) => bytes,
        None => {
            runner
                .run(key, &probe.produce_source)
                .map_err(|message| ProbeError {
                    key: key.to_string(),
                    message,
                })?
                .bytes
        }
    };

    let recorded = record(key, ctx, &bytes);

    Ok(Evaluated {
        bytes,
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
