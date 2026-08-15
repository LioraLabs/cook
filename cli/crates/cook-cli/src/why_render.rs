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
            CacheStatus::PinnedColdMiss => {
                "MISS (local), MISS (shared) — pinned, fetch-only".to_string()
            }
            // CS-0173: name the upstream, not the symptom. This unit does not
            // "miss" in any cache sense; it has no key yet to hit or miss with.
            CacheStatus::ForcedByUpstream { producer, .. } => {
                format!("REBUILD (forced by {producer})")
            }
            _ => {
                let local = if u.local_hit {
                    "HIT (local)"
                } else {
                    "MISS (local)"
                };
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
        s.push_str(&format!(
            "  command_hash      {:016x}\n",
            u.determinants.command_hash
        ));
        s.push_str(&format!(
            "  env_contribution  {:016x}\n",
            u.determinants.env_contribution
        ));
        s.push_str(&format!(
            "  seal_contribution {:016x}\n",
            u.determinants.seal_contribution
        ));
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
                s.push_str(&format!("    {k} = {v}{}\n", tools_probe_paths(v)));
            }
        }
        // CS-0174: the local tier's answer to the question the shared tier
        // answers with a manifest diff. Printed before it, because a unit that
        // misses locally is asking "what changed since I last ran this" and
        // that is the nearer question.
        if let Some(cause) = &u.local_cause {
            s.push_str(&format!("  local-miss cause: {cause}\n"));
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
                if matches!(
                    u.status,
                    CacheStatus::SharedMiss | CacheStatus::PinnedColdMiss
                ) {
                    s.push_str("  shared-miss diff: no producer manifest published for this key\n");
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
        Probe { key, ours, theirs } => {
            format!("probe {key}: ours {ours:?} != producer {theirs:?}")
        }
        OutputPaths { ours, theirs } => format!("outputs: ours {ours:?} != producer {theirs:?}"),
    }
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
    let units: Vec<serde_json::Value> = report
        .units
        .iter()
        .map(|u| why_unit_json(u, timings))
        .collect();
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
    });

    let manifest_diff = match &u.manifest_diff {
        None => serde_json::Value::Null,
        Some(diffs) => serde_json::Value::Array(diffs.iter().map(determinant_diff_json).collect()),
    };

    let mut obj = serde_json::Map::new();
    obj.insert(
        "recipe".to_string(),
        serde_json::Value::String(u.recipe_name.clone()),
    );
    obj.insert(
        "cache_key".to_string(),
        serde_json::Value::String(u.cache_key.clone()),
    );
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
    obj.insert(
        "status".to_string(),
        serde_json::Value::String(status_str.to_string()),
    );
    for (k, v) in status_obj {
        obj.insert(k, v);
    }
    // COOK-276: explicit per-tier answers alongside the legacy single status.
    obj.insert(
        "local_hit".to_string(),
        serde_json::Value::Bool(u.local_hit),
    );
    obj.insert(
        "shared_present".to_string(),
        match u.shared_present {
            Some(b) => serde_json::Value::Bool(b),
            None => serde_json::Value::Null,
        },
    );
    obj.insert(
        "disposition".to_string(),
        serde_json::Value::String(disposition.to_string()),
    );
    obj.insert("determinants".to_string(), determinants);
    obj.insert("manifest_diff".to_string(), manifest_diff);
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
        Probe { key, ours, theirs } => serde_json::json!({
            "determinant": format!("probe:{key}"),
            "ours": stropt(ours),
            "producer": stropt(theirs),
        }),
        OutputPaths { ours, theirs } => serde_json::json!({
            "determinant": "output_paths",
            "ours": ours.clone(),
            "producer": theirs.clone(),
        }),
    }
}
