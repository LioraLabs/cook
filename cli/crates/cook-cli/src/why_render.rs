//! Turning a `cook why` report into bytes.
//!
//! One thing: the query's answer, prepared for a reader. `pipeline.rs`
//! registers the workspace and asks `cook_engine::why::explain` what a run
//! would do; this module is everything that happens to the answer afterwards —
//! the plain text a person reads, the JSON a tool parses, the two `--level` /
//! `--format` words that select between them, and `annotations_from`, which
//! folds the report into the per-unit facts `cook-graph` aggregates over. That
//! last one produces no bytes of its own, and it belongs here anyway: it is the
//! report restated for a renderer, and the alternative is `pipeline.rs` reading
//! `local_hit` and `shared_present` to decide what "served" means, which is one
//! more place holding an opinion about a cache verdict.
//!
//! It holds no cache logic and performs no lookup. Every verdict it prints was
//! decided in `cook-engine`; a `match` here that reached its own conclusion
//! would be a second classifier, and `cook why`'s whole value is that the key
//! it reports is the key a run would compute.
//!
//! The one exception is deliberate and documented at `tools_probe_paths`: a
//! tool's path is resolved fresh at render time, because the sealed value
//! carries identity and not location (CS-0157).
//!
//! Tested by `cli/crates/cook-cli/tests/why_render.rs`, which drives the real
//! binary rather than these functions — the rendering is a surface, and a
//! surface is worth testing where the user meets it.

use cook_contracts::probe_value::ProbeDelta;

use crate::error::CookError;

pub(crate) fn parse_level(s: &str) -> Result<cook_graph::emit::Level, CookError> {
    match s {
        "recipe" => Ok(cook_graph::emit::Level::Recipe),
        "group" => Ok(cook_graph::emit::Level::Group),
        "unit" => Ok(cook_graph::emit::Level::Unit),
        other => Err(CookError::Other(format!(
            "unknown --level '{other}'; expected recipe, group, or unit"
        ))),
    }
}

pub(crate) fn parse_format(s: &str) -> Result<cook_graph::emit::Format, CookError> {
    match s {
        "text" => Ok(cook_graph::emit::Format::Text),
        "mermaid" => Ok(cook_graph::emit::Format::Mermaid),
        "dot" => Ok(cook_graph::emit::Format::Dot),
        "json" => Ok(cook_graph::emit::Format::Json),
        other => Err(CookError::Other(format!(
            "unknown --format '{other}'; expected text, mermaid, dot, or json"
        ))),
    }
}

/// Fold the determinant report and recorded observations into the fact map the
/// graph aggregates over.
///
/// "Served" is the union of both tiers, deliberately: a locally-warm unit does
/// not rebuild merely because the shared store has never heard of it, and
/// COOK-276 exists because conflating the two read as "this will rebuild".
pub(crate) fn annotations_from(
    report: &cook_engine::why::WhyReport,
    timings: &cook_engine::observations::Observations,
) -> cook_graph::Annotations {
    let mut a = cook_graph::Annotations::new();
    for u in &report.units {
        a.insert(
            &u.recipe_name,
            &u.cache_key,
            cook_graph::UnitFacts {
                served: u.local_hit || u.shared_present == Some(true),
                observed_ms: timings
                    .get(&u.recipe_name, &u.cache_key)
                    .map(|o| o.elapsed_ms),
            },
        );
    }
    a
}

/// `--unit <pattern>`: full determinants for the units whose recipe name,
/// cache key, or output paths contain `pattern`.
pub(crate) fn render_selected_units(
    report: &cook_engine::why::WhyReport,
    pattern: &str,
    format: cook_graph::emit::Format,
    timings: &cook_engine::observations::Observations,
) -> Result<(), CookError> {
    let matched: Vec<_> = report
        .units
        .iter()
        .filter(|u| {
            u.recipe_name.contains(pattern)
                || u.cache_key.contains(pattern)
                || u.determinants
                    .output_paths
                    .iter()
                    .any(|p| p.contains(pattern))
        })
        .cloned()
        .collect();
    if matched.is_empty() {
        // A selector that matches nothing is a user error worth naming, not an
        // empty report that reads as "nothing to explain".
        return Err(CookError::Other(format!(
            "--unit '{pattern}' matched no unit in {}'s closure; \
             run without --unit to see the units available",
            report.recipe
        )));
    }
    let selected = cook_engine::why::WhyReport {
        recipe: report.recipe.clone(),
        units: matched,
    };
    match format {
        cook_graph::emit::Format::Json => print!("{}", render_why_json(&selected, timings)),
        _ => print!("{}", render_why_plain(&selected, timings)),
    }
    Ok(())
}

pub(crate) fn render_why_plain(
    report: &cook_engine::why::WhyReport,
    timings: &cook_engine::observations::Observations,
) -> String {
    use cook_engine::why::CacheStatus;
    let mut s = String::new();
    s.push_str(&format!("why {}\n", report.recipe));
    for u in &report.units {
        // COOK-276: label both tiers explicitly. A bare `MISS (shared)` on a
        // locally-warm unit reads as "this will rebuild" when it only means
        // "absent from the shared store tier".
        let status = match &u.status {
            CacheStatus::MissingInput { path } => format!("MISS (input '{path}' missing)"),
            CacheStatus::PinnedColdMiss => "MISS (local), MISS (shared) — pinned, fetch-only".to_string(),
            // CS-0173: name the upstream, not the symptom. This unit does not
            // "miss" in any cache sense; it has no key yet to hit or miss with.
            CacheStatus::ForcedByUpstream { producer, .. } => {
                format!("REBUILD (forced by {producer})")
            }
            CacheStatus::UnmaterialisedProbe { key } => {
                format!("KEY NOT COMPUTABLE (probe '{key}' unmaterialised)")
            }
            _ => {
                let local = if u.local_hit { "HIT (local)" } else { "MISS (local)" };
                match u.shared_present {
                    None => local.to_string(),
                    Some(true) => format!("{local}, HIT (shared)"),
                    Some(false) => format!("{local}, MISS (shared)"),
                }
            }
        };
        // CS-0173: print no key for a forced unit. There is no honest number to
        // put here, and printing one would suggest a lookup that never happened.
        let key_field = if u.key_hex.is_empty() {
            "key not computable until then".to_string()
        } else {
            format!("key {}", u.key_hex)
        };
        s.push_str(&format!(
            "\n{} :: {} [{}]  {}\n",
            u.recipe_name, u.cache_key, status, key_field
        ));
        s.push_str(&format!("  command_hash      {:016x}\n", u.determinants.command_hash));
        s.push_str(&format!("  env_contribution  {:016x}\n", u.determinants.env_contribution));
        s.push_str(&format!("  seal_contribution {:016x}\n", u.determinants.seal_contribution));
        if !u.determinants.inputs.is_empty() {
            s.push_str("  inputs:\n");
            for (p, h) in &u.determinants.inputs {
                s.push_str(&format!("    {p}  {h:016x}\n"));
            }
        }
        // CS-0173: shown separately from `inputs` and without a hash, because
        // there is no hash yet — the producing unit has not run.
        if !u.determinants.pending_inputs.is_empty() {
            s.push_str("  inputs (not determined yet):\n");
            for (p, producer) in &u.determinants.pending_inputs {
                s.push_str(&format!("    {p}  pending {producer}\n"));
            }
        }
        if !u.determinants.output_paths.is_empty() {
            s.push_str("  outputs:\n");
            for p in &u.determinants.output_paths {
                s.push_str(&format!("    {p}\n"));
            }
        }
        if !u.determinants.consulted_env.is_empty() {
            s.push_str("  env (consulted):\n");
            for (k, v) in &u.determinants.consulted_env {
                s.push_str(&format!("    {k} = {v}\n"));
            }
        }
        if !u.determinants.sealed_probes.is_empty() {
            s.push_str("  sealed probes:\n");
            for (k, v) in &u.determinants.sealed_probes {
                // §17.1.6.1: a probe with no materialised value at all MUST be
                // reported as unmaterialised, never as "produced" nor
                // conflated with case (c)'s "reported from a prior
                // invocation" (I-2, seam review on 9eb2faf7).
                // `resolve_sealed_probes` (seal.rs) folds an absent value to
                // the empty string, and a materialised probe value is
                // canonical JSON, which is never the empty string — so
                // `v.is_empty()` is a safe, cheap discriminator between "no
                // value at all" and "a value this query didn't resolve
                // fresh".
                //
                // §17.1.6.1: a value the query could not resolve fresh (case
                // c, or a fresh-resolution failure that fell back) MUST be
                // identified as reported from a prior invocation rather than
                // resolved now. A short marker, not a second report.
                let marker = if v.is_empty() {
                    "  [unmaterialised]".to_string()
                } else if u.determinants.prior_invocation_probes.contains(k) {
                    "  [prior invocation]".to_string()
                } else {
                    String::new()
                };
                // §17.1.6.1: a fresh-resolution failure MUST be reported
                // against the probe it belongs to.
                let failure = match u.determinants.probe_lookup_failures.get(k) {
                    Some(msg) => format!("  [fresh resolution failed: {msg}]"),
                    None => String::new(),
                };
                // M-1 (seam review on 9eb2faf7): the marker sits BEFORE `=`.
                // An object value pretty-prints across several lines
                // (`serde_json`'s default `Display` for a JSON object is not
                // one-line), so a marker appended AFTER `{v}` landed alone
                // under the value's closing `}` and read as annotating the
                // NEXT field rather than this one.
                s.push_str(&format!(
                    "    {k}{marker}{failure} = {v}{}\n",
                    tools_probe_paths(v)
                ));
            }
        }
        // CS-0174: the local tier's answer to the question the shared tier
        // answers with a manifest diff. Printed before it, because a unit that
        // misses locally is asking "what changed since I last ran this" and
        // that is the nearer question.
        if let Some(cause) = &u.local_cause {
            s.push_str(&format!("  local-miss cause: {cause}\n"));
            // CS-0245 / §17.1.6.1: when the diverging determinant is the seal
            // set, name the probe(s) that moved instead of leaving the reader
            // to go dig for it. `seal_deltas` is `Some` exactly when the
            // engine matched `RebuildReason::SealChanged` on the variant
            // itself — this renderer never re-derives that from `cause`'s
            // text, which would be a second copy of the same decision.
            match &u.seal_deltas {
                None => {}
                Some(deltas) if deltas.is_empty() => {
                    // §17.1.6.1's I5: the local per-probe record moves on every
                    // REACH, the unit's index record only on its last EXECUTION —
                    // two different events, so a genuine seal-changed miss MAY
                    // leave nothing in `.cook/probes/` to diff against. The
                    // Standard requires naming the seal set as the differing
                    // determinant WITHOUT naming a probe key or an entry here,
                    // and forbids presenting this as an empty or partial diff.
                    // M-2 (seam review on 9eb2faf7): the parenthetical names
                    // ONE cause of an empty diff (the local per-probe record
                    // moving on every reach while the unit's index record
                    // moves only on last execution) but this branch is also
                    // reached when `.cook/probes/` is simply absent — deleted
                    // between the build and the query — where that cause is
                    // false. Prefixed as an example rather than asserted as
                    // the reason.
                    s.push_str(
                        "    seal set differs from the local record; no probe or \
                         entry can be named for this miss (for example, the local \
                         per-probe record moves whenever a probe is reached, while \
                         the unit's index record moves only when this unit last \
                         ran)\n",
                    );
                }
                Some(deltas) => {
                    for (key, delta) in deltas {
                        s.push_str(&format!("    {key}:\n"));
                        s.push_str(&render_probe_delta_lines(delta, "      "));
                    }
                }
            }
        }
        // CS-0174: history, kept plainly separate from the live verdict above.
        // For a unit that is currently a hit this is the only causal answer
        // available, and it is the one that answers "why did this rebuild
        // overnight when I changed nothing".
        if let Some(obs) = timings.get(&u.recipe_name, &u.cache_key) {
            if let Some(cause) = &obs.cause {
                s.push_str(&format!(
                    "  last ran because: {cause} (recorded at Unix {})\n",
                    obs.recorded_at
                ));
            }
            s.push_str(&format!("  recorded output: {} bytes\n", obs.log_bytes));
        }
        match &u.manifest_diff {
            Some(diffs) if diffs.is_empty() => {
                s.push_str(
                    "  shared-miss diff: producer manifest determinants are identical to ours \
                     (artifact not published, or absent from this backend)\n",
                );
            }
            Some(diffs) => {
                s.push_str("  shared-miss diff vs producer manifest:\n");
                for d in diffs {
                    s.push_str(&format!("    {}\n", render_diff(d)));
                }
            }
            None => {
                if matches!(u.status, CacheStatus::SharedMiss | CacheStatus::PinnedColdMiss) {
                    s.push_str(
                        "  shared-miss diff: no producer manifest published for this key\n",
                    );
                }
            }
        }
    }
    s
}

/// Extract a " (cc→/usr/bin/cc, …)" suffix for a tools-probe JSON value
/// ({"NAME":{"hash":...}}). Empty for non-tools probe values.
///
/// CS-0157: the sealed value carries identity only (content hash) — path is
/// location metadata and is resolved FRESH at query time, so the display
/// shows where each tool resolves now rather than where it lived when the
/// value was produced. A tool no longer on PATH annotates as `<not found>`.
fn tools_probe_paths(value: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(value) else {
        return String::new();
    };
    let Some(obj) = v.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (name, entry) in obj {
        let tool_shaped = entry
            .as_object()
            .is_some_and(|e| e.get("hash").is_some_and(|h| h.is_string()));
        if tool_shaped {
            match cook_engine::why::resolve_tool_path(name) {
                Some(p) => parts.push(format!("{name}→{p}")),
                None => parts.push(format!("{name}→<not found>")),
            }
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  ({})", parts.join(", "))
    }
}

fn render_diff(d: &cook_engine::why::DeterminantDiff) -> String {
    use cook_engine::why::DeterminantDiff::*;
    match d {
        CommandHash { ours, theirs } => {
            format!("command_hash: ours {ours:016x} != producer {theirs:016x}")
        }
        EnvContribution { ours, theirs } => {
            format!("env_contribution: ours {ours:016x} != producer {theirs:016x}")
        }
        SealContribution { ours, theirs } => {
            format!("seal_contribution: ours {ours:016x} != producer {theirs:016x}")
        }
        Input { path, ours, theirs } => {
            format!("input {path}: ours {ours:?} != producer {theirs:?}")
        }
        Env { key, ours, theirs } => format!("env {key}: ours {ours:?} != producer {theirs:?}"),
        // CS-0245 / §17.1.6.1: the shared-miss manifest diff names a moved
        // probe the same way the local-miss diff does — one attribution
        // obligation, read at both tiers — instead of dumping `ours`/`theirs`
        // whole through `Debug`. `theirs` (the producer manifest's recorded
        // value) is the "old" side; `ours` (this consumer's freshly resolved
        // value) is "new".
        Probe { key, ours, theirs } => {
            let current = ours.as_deref().unwrap_or("");
            let prior = theirs.as_deref().map(str::as_bytes);
            match cook_contracts::probe_value::probe_delta(prior, current.as_bytes()) {
                // M-6 (seam review on 9eb2faf7): `ProbeDelta::FirstObservation`
                // only arises here when `theirs` (the producer MANIFEST's own
                // recorded value) was `None` — a fact about the SHARED tier,
                // not about this invocation's local forensic record. Passing
                // it through `render_probe_delta_lines`'s generic wording
                // ("first observation (no prior recorded value)") reads as the
                // local-miss meaning of that phrase — "no prior invocation
                // wrote a `.cook/probes/` record" — which is a different and
                // false claim on this tier: the producer simply never
                // published a value for this key. Give the shared arm its own
                // label instead of borrowing the local one.
                Some(ProbeDelta::FirstObservation) => {
                    format!("probe {key}: producer manifest has no recorded value for this key")
                }
                Some(delta) => {
                    let lines = render_probe_delta_lines(&delta, "      ");
                    format!("probe {key}:\n{}", lines.trim_end_matches('\n'))
                }
                // Unreachable in practice (this arm only fires when `ours !=
                // theirs` as strings, which for canonical-JSON bytes means the
                // bytes differ too), but a defensive arm is cheaper than a
                // panic if the two ever diverge.
                None => format!("probe {key}: (no byte-level difference found)"),
            }
        }
        OutputPaths { ours, theirs } => format!("outputs: ours {ours:?} != producer {theirs:?}"),
    }
}

/// Render one probe's delta as indented lines — the single law both the
/// local-miss block above and the shared-miss `Probe` diff arm read through
/// (CS-0245 / §17.1.6.1: two tiers, one rendering).
fn render_probe_delta_lines(delta: &ProbeDelta, indent: &str) -> String {
    let mut s = String::new();
    match delta {
        ProbeDelta::FirstObservation => {
            s.push_str(&format!("{indent}first observation (no prior recorded value)\n"));
        }
        ProbeDelta::Entries { added, removed, changed } => {
            for (k, v) in added {
                s.push_str(&format!("{indent}+ {k}: {}\n", abbreviate(v)));
            }
            for (k, v) in removed {
                s.push_str(&format!("{indent}- {k}: {}\n", abbreviate(v)));
            }
            for (k, old, new) in changed {
                s.push_str(&format!(
                    "{indent}{k}: {} -> {}\n",
                    abbreviate(old),
                    abbreviate(new)
                ));
            }
        }
        ProbeDelta::Value { old, new } => {
            s.push_str(&format!("{indent}{} -> {}\n", abbreviate(old), abbreviate(new)));
        }
    }
    s
}

/// A prefix long enough that a truncated sha256 hex hash (64 chars) still
/// reads as distinct from any other hash a build is realistically dealing
/// with in one report; not a cryptographic collision bound, just a display
/// heuristic. Shorten further only after checking this module's tests don't
/// start rendering two different values as the same abbreviation.
const ABBREV_LEN: usize = 12;

/// Values shorter than this print in full — abbreviating a 15-char string to
/// 12 chars plus an ellipsis saves nothing and just adds noise.
const ABBREV_THRESHOLD: usize = 20;

/// Abbreviate a long value (a 64-hex hash is the case this exists for) to a
/// stable prefix so a changed value reads at a glance. Both sides of a delta
/// are abbreviated identically by construction — this is the only function
/// either side goes through.
fn abbreviate(v: &str) -> String {
    if v.chars().count() <= ABBREV_THRESHOLD {
        return v.to_string();
    }
    let prefix: String = v.chars().take(ABBREV_LEN).collect();
    format!("{prefix}\u{2026}")
}

// Deterministic JSON output (sorted object keys, §17.1.6 informative note)
// relies on `serde_json` being built WITHOUT the `preserve_order` feature, so
// `serde_json::Map` is BTreeMap-backed and serialises keys sorted regardless of
// insertion order. The determinant maps themselves are already `BTreeMap` in the
// engine; this note covers the per-unit object keys assembled here.
//
// CS-0217: the version is stamped by `cook_graph::stamp_schema_version`, the
// same call the whole-closure document goes through, because this IS that
// document at reduced scope — same `units` array, built by the same
// `why_unit_json`. Neither the key nor the number is spelled here: a renderer
// that wrote `"schema_version"` itself would be the second end of a wire
// format with nothing holding the two ends together.
fn render_why_json(
    report: &cook_engine::why::WhyReport,
    timings: &cook_engine::observations::Observations,
) -> String {
    let units: Vec<serde_json::Value> =
        report.units.iter().map(|u| why_unit_json(u, timings)).collect();
    let mut document = serde_json::json!({
        "recipe": report.recipe,
        "units": units,
    });
    cook_graph::stamp_schema_version(&mut document);
    serde_json::to_string_pretty(&document).unwrap_or_default() + "\n"
}

pub(crate) fn why_unit_json(
    u: &cook_engine::why::WhyUnit,
    timings: &cook_engine::observations::Observations,
) -> serde_json::Value {
    use cook_engine::why::CacheStatus;

    let mut status_obj = serde_json::Map::new();
    let status_str = match &u.status {
        CacheStatus::LocalHit => "local_hit",
        CacheStatus::SharedHit => "shared_hit",
        CacheStatus::SharedMiss => "shared_miss",
        CacheStatus::LocalOnlyMiss => "local_only_miss",
        CacheStatus::PinnedColdMiss => "pinned_cold_miss",
        CacheStatus::MissingInput { path } => {
            status_obj.insert(
                "missing_input_path".to_string(),
                serde_json::Value::String(path.clone()),
            );
            "missing_input"
        }
        // CS-0173: no `key` field is emitted for this status (see below) — the
        // unit's key is not computable, and a consumer must be able to tell that
        // apart from a key that happens to miss.
        CacheStatus::ForcedByUpstream { producer, path } => {
            status_obj.insert(
                "forced_by".to_string(),
                serde_json::Value::String(producer.clone()),
            );
            status_obj.insert(
                "pending_input_path".to_string(),
                serde_json::Value::String(path.clone()),
            );
            "forced_by_upstream"
        }
        CacheStatus::UnmaterialisedProbe { key } => {
            status_obj.insert(
                "unmaterialised_probe".to_string(),
                serde_json::Value::String(key.clone()),
            );
            "unmaterialised_probe"
        }
    };

    let disposition = match u.disposition {
        cook_engine::why::Disposition::Unannotated => "unannotated",
        cook_engine::why::Disposition::Local => "local",
        cook_engine::why::Disposition::Pinned => "pinned",
    };

    let inputs: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .inputs
        .iter()
        .map(|(p, h)| (p.clone(), serde_json::Value::String(format!("{h:016x}"))))
        .collect();

    let consulted_env: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .consulted_env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    let sealed_probes: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .sealed_probes
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    let pending_inputs: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .pending_inputs
        .iter()
        .map(|(p, producer)| (p.clone(), serde_json::Value::String(producer.clone())))
        .collect();

    // CS-0245 / §17.1.6.1: a sealed key `cook why` could not resolve fresh —
    // reported from a prior invocation rather than resolved now — MUST be
    // identified as such, and a fresh-resolution failure MUST be reported
    // against the probe it belongs to. Additive siblings of `sealed_probes`
    // rather than a reshape of it, so a consumer that ignores unknown keys
    // sees no change (CS-0245: additive, no schema bump).
    let prior_invocation_probes: Vec<serde_json::Value> = u
        .determinants
        .prior_invocation_probes
        .iter()
        .cloned()
        .map(serde_json::Value::String)
        .collect();
    let probe_lookup_failures: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .probe_lookup_failures
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    let determinants = serde_json::json!({
        "command_hash": format!("{:016x}", u.determinants.command_hash),
        "env_contribution": format!("{:016x}", u.determinants.env_contribution),
        "seal_contribution": format!("{:016x}", u.determinants.seal_contribution),
        "inputs": serde_json::Value::Object(inputs),
        // CS-0173: path → producing recipe, for inputs whose content is not
        // determined yet. Disjoint from `inputs` by construction.
        "pending_inputs": serde_json::Value::Object(pending_inputs),
        "output_paths": u.determinants.output_paths.clone(),
        "consulted_env": serde_json::Value::Object(consulted_env),
        "sealed_probes": serde_json::Value::Object(sealed_probes),
        "prior_invocation_probes": prior_invocation_probes,
        "probe_lookup_failures": serde_json::Value::Object(probe_lookup_failures),
    });

    let manifest_diff = match &u.manifest_diff {
        None => serde_json::Value::Null,
        Some(diffs) => serde_json::Value::Array(diffs.iter().map(determinant_diff_json).collect()),
    };

    let mut obj = serde_json::Map::new();
    obj.insert("recipe".to_string(), serde_json::Value::String(u.recipe_name.clone()));
    obj.insert("cache_key".to_string(), serde_json::Value::String(u.cache_key.clone()));
    // CS-0173: a forced unit has no computable key, and the wire format says so
    // with null rather than an empty string, so a consumer cannot mistake
    // "not computable" for "computed, and it is the empty key".
    obj.insert(
        "key".to_string(),
        if u.key_hex.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(u.key_hex.clone())
        },
    );
    obj.insert("line".to_string(), serde_json::json!(u.line));
    obj.insert("status".to_string(), serde_json::Value::String(status_str.to_string()));
    for (k, v) in status_obj {
        obj.insert(k, v);
    }
    // COOK-276: explicit per-tier answers alongside the legacy single status.
    obj.insert("local_hit".to_string(), serde_json::Value::Bool(u.local_hit));
    obj.insert(
        "shared_present".to_string(),
        match u.shared_present {
            Some(b) => serde_json::Value::Bool(b),
            None => serde_json::Value::Null,
        },
    );
    obj.insert("disposition".to_string(), serde_json::Value::String(disposition.to_string()));
    obj.insert("determinants".to_string(), determinants);
    obj.insert("manifest_diff".to_string(), manifest_diff);
    // CS-0245 / §17.1.6.1: structured data, not the rendered string — a
    // consumer parses `kind` and the entry arrays rather than scraping text.
    // Empty array when there is no delta to report (including local hits and
    // §17.1.6.1's I5 empty-diff case, which `local_cause` already names).
    obj.insert(
        "seal_deltas".to_string(),
        serde_json::Value::Array(
            // `Option<Vec<_>>`: `None` (not a seal-changed miss) and
            // `Some(vec![])` (§17.1.6.1's I5 — a genuine seal-changed miss
            // with nothing nameable) both render as `[]`; `local_cause`
            // already carries the distinction between them.
            u.seal_deltas
                .iter()
                .flatten()
                .map(|(key, delta)| {
                    let mut entry = probe_delta_json(delta);
                    if let serde_json::Value::Object(map) = &mut entry {
                        map.insert("key".to_string(), serde_json::Value::String(key.clone()));
                    }
                    entry
                })
                .collect(),
        ),
    );
    // CS-0174: local-tier attribution, the counterpart of `manifest_diff`.
    obj.insert(
        "local_cause".to_string(),
        match &u.local_cause {
            Some(c) => serde_json::Value::String(c.clone()),
            None => serde_json::Value::Null,
        },
    );
    // CS-0174: history, and labelled as history. `last_cause` says why the unit
    // ran on a past build; `local_cause` says why it will run now. A consumer
    // must not read one for the other, so they are separate keys and the age of
    // the observation rides alongside.
    let last = timings.get(&u.recipe_name, &u.cache_key);
    obj.insert(
        "last_cause".to_string(),
        match last.and_then(|o| o.cause.as_ref()) {
            Some(c) => serde_json::Value::String(c.clone()),
            None => serde_json::Value::Null,
        },
    );
    obj.insert(
        "last_cause_recorded_at".to_string(),
        match last.filter(|o| o.cause.is_some()) {
            Some(o) => serde_json::json!(o.recorded_at),
            None => serde_json::Value::Null,
        },
    );
    obj.insert(
        "recorded_log_bytes".to_string(),
        last.map(|o| serde_json::json!(o.log_bytes))
            .unwrap_or(serde_json::Value::Null),
    );
    serde_json::Value::Object(obj)
}

/// `ProbeDelta` as structured JSON — the wire form of the one delta law, read
/// by both `why_unit_json`'s `seal_deltas` array and `determinant_diff_json`'s
/// `Probe` arm. Full-fidelity values (unabbreviated): abbreviation is a
/// plain-text display concern only, and a JSON consumer should see the exact
/// bytes.
fn probe_delta_json(delta: &ProbeDelta) -> serde_json::Value {
    match delta {
        ProbeDelta::FirstObservation => serde_json::json!({ "kind": "first_observation" }),
        ProbeDelta::Entries { added, removed, changed } => serde_json::json!({
            "kind": "entries",
            "added": added.iter()
                .map(|(k, v)| serde_json::json!({"key": k, "value": v}))
                .collect::<Vec<_>>(),
            "removed": removed.iter()
                .map(|(k, v)| serde_json::json!({"key": k, "value": v}))
                .collect::<Vec<_>>(),
            "changed": changed.iter()
                .map(|(k, old, new)| serde_json::json!({"key": k, "old": old, "new": new}))
                .collect::<Vec<_>>(),
        }),
        ProbeDelta::Value { old, new } => serde_json::json!({
            "kind": "value",
            "old": old,
            "new": new,
        }),
    }
}

fn determinant_diff_json(d: &cook_engine::why::DeterminantDiff) -> serde_json::Value {
    use cook_engine::why::DeterminantDiff::*;
    let hexopt = |o: &Option<u64>| match o {
        Some(h) => serde_json::Value::String(format!("{h:016x}")),
        None => serde_json::Value::Null,
    };
    let stropt = |o: &Option<String>| match o {
        Some(s) => serde_json::Value::String(s.clone()),
        None => serde_json::Value::Null,
    };
    match d {
        CommandHash { ours, theirs } => serde_json::json!({
            "determinant": "command_hash",
            "ours": format!("{ours:016x}"),
            "producer": format!("{theirs:016x}"),
        }),
        EnvContribution { ours, theirs } => serde_json::json!({
            "determinant": "env_contribution",
            "ours": format!("{ours:016x}"),
            "producer": format!("{theirs:016x}"),
        }),
        SealContribution { ours, theirs } => serde_json::json!({
            "determinant": "seal_contribution",
            "ours": format!("{ours:016x}"),
            "producer": format!("{theirs:016x}"),
        }),
        Input { path, ours, theirs } => serde_json::json!({
            "determinant": format!("input:{path}"),
            "ours": hexopt(ours),
            "producer": hexopt(theirs),
        }),
        Env { key, ours, theirs } => serde_json::json!({
            "determinant": format!("env:{key}"),
            "ours": stropt(ours),
            "producer": stropt(theirs),
        }),
        // CS-0245: the `Probe` diff entry gains the structured delta ALONGSIDE
        // its existing `ours`/`producer` fields rather than replacing them, so
        // a consumer that ignores unknown keys sees no change.
        Probe { key, ours, theirs } => {
            let current = ours.as_deref().unwrap_or("");
            let prior = theirs.as_deref().map(str::as_bytes);
            let delta = cook_contracts::probe_value::probe_delta(prior, current.as_bytes());
            let mut obj = serde_json::json!({
                "determinant": format!("probe:{key}"),
                "ours": stropt(ours),
                "producer": stropt(theirs),
            });
            if let Some(d) = delta {
                if let serde_json::Value::Object(map) = &mut obj {
                    map.insert("delta".to_string(), probe_delta_json(&d));
                }
            }
            obj
        }
        OutputPaths { ours, theirs } => serde_json::json!({
            "determinant": "output_paths",
            "ours": ours.clone(),
            "producer": theirs.clone(),
        }),
    }
}

#[cfg(test)]
#[path = "tests/why_render_tests.rs"]
mod tests;
