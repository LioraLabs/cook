//! Lower the planned unit graph into the `Dag<WorkNode>` the executor runs.
//!
//! The wiring decision — which units become nodes and which dependencies
//! each node has (barriers, step groups, probe consumption and pruning,
//! synthesised top-level probes, coarse `deps` vs fine `dep_edges`, leaf
//! pass-through across unit-less meta-targets) — is NOT made here. It is
//! `cook_contracts::unit_graph::plan`, the shared law this crate and
//! `cook-graph` both consume, so the DAG that executes and the graph `cook
//! why` renders cannot disagree (COOK-402 / CS-0202).
//!
//! What this module owns is execution policy over that plan: constructing
//! each node's `WorkNode` (payload, env merging, the presatisfied rule) and
//! the plan-time structural checks that gate execution — output collisions
//! (§16.1.1), the terminal-output rule (§22.1.2), and literal
//! read-after-write (§16.1.2).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use cook_contracts::unit_graph::{self, NodeOrigin, UnitGraphError};
use cook_contracts::{CapturedUnit, RecipeUnits, WorkPayload};
use cook_dag::Dag;

use crate::{EngineError, WorkNode};

impl From<UnitGraphError> for EngineError {
    fn from(e: UnitGraphError) -> Self {
        match e {
            UnitGraphError::DanglingDepEdge {
                referring_recipe,
                dep_name,
            } => EngineError::DanglingDepEdge {
                referring_recipe,
                dep_name,
            },
            UnitGraphError::Cycle { unresolved } => {
                EngineError::CycleDetected(format!("cycle among recipes: {unresolved:?}"))
            }
        }
    }
}

/// Build the executable `Dag<WorkNode>` for a topologically-sorted closure.
///
/// **Unified-call contract (SHI-222).** The caller passes _every_ reachable
/// recipe in a single invocation, in topological order —
/// `cook_contracts::unit_graph::toposort_recipes` produces one. The wiring
/// itself (including the rejection of dangling `dep_edges`) is
/// [`unit_graph::plan`]'s; see that module for the rules. This function adds
/// the engine's plan-time gate and the lowering:
///
/// - **§16.1.1 output collisions.** Two recipes with no dependency path
///   between them declaring the same canonical output path race silently
///   under `--jobs > 1`; the plan is rejected before any work is dispatched.
/// - **Lowering.** Each planned node becomes a `WorkNode`: payload, merged
///   env, and the presatisfied rule for captured units; a synthesised
///   `WorkPayload::Probe` from `ru.probes` metadata for top-level probes.
///   Dependency ids are deduplicated (the same producer may back both a
///   coarse barrier and a fine `dep_order` ref — the additive rule — but the
///   schedule needs the id once).
pub fn build_dag(recipe_units: Vec<RecipeUnits>) -> Result<Dag<WorkNode>, EngineError> {
    // ── Plan-time output-collision check ─────────────────────────────────────
    if let Some(err) = detect_output_collisions(&recipe_units) {
        return Err(err);
    }

    // ── The wiring decision, made once, in cook-contracts ────────────────────
    let plan = unit_graph::plan(&recipe_units)?;

    let units_by_name: BTreeMap<&str, &RecipeUnits> = recipe_units
        .iter()
        .map(|ru| (ru.recipe_name.as_str(), ru))
        .collect();

    // ── Lowering ─────────────────────────────────────────────────────────────
    let mut dag = Dag::new();
    for (idx, node) in plan.nodes.iter().enumerate() {
        let mut dep_ids: Vec<usize> = Vec::new();
        for &(d, _) in &node.deps {
            if !dep_ids.contains(&d) {
                dep_ids.push(d);
            }
        }

        let work_node = match &node.origin {
            NodeOrigin::Unit { recipe, unit_idx } => {
                let ru = units_by_name
                    .get(recipe.as_str())
                    .expect("plan origins name recipes from the input slice");
                lower_unit(ru, &ru.units[*unit_idx])
            }
            NodeOrigin::SynthProbe { recipe, probe_key } => {
                let ru = units_by_name
                    .get(recipe.as_str())
                    .expect("plan origins name recipes from the input slice");
                let meta = ru
                    .probes
                    .iter()
                    .find(|p| p.key == *probe_key)
                    .expect("plan synthesises only keys with metadata in ru.probes");
                WorkNode {
                    payload: Some(WorkPayload::Probe {
                        key: meta.key.clone(),
                        produce: meta.produce_source.clone(),
                        line: meta.produce_line,
                    }),
                    recipe_name: ru.recipe_name.clone(),
                    cache_meta: None,
                    test_name: None,
                    member: None,
                    working_dir: ru.working_dir.clone(),
                    env_vars: ru.env_vars.clone(),
                    // A probe runs a Lua `produce` body (reads `cook.env` via
                    // the full map); it spawns no shell step, so its
                    // process-env subset is empty.
                    process_env_vars: BTreeMap::new(),
                }
            }
        };

        // Builder invariant: plan deps only reference earlier nodes, so the
        // call cannot fail with `DependencyOutOfRange`; and the dag id
        // mirrors the plan index because nodes are added 1:1 in plan order.
        let dag_id = dag
            .add_node(work_node, &dep_ids)
            .expect("plan deps reference earlier nodes only");
        debug_assert_eq!(dag_id, idx, "dag ids must mirror plan node indices");
    }

    Ok(dag)
}

/// Lower one captured unit into its `WorkNode`.
fn lower_unit(ru: &RecipeUnits, unit: &CapturedUnit) -> WorkNode {
    // Per-unit env vars (e.g. chore param exports, COOK-36 §7.1.2) are
    // merged on top of the recipe-level env vars. Unit env wins on conflicts
    // (params shadow any recipe-level key of the same name).
    let merged_env_vars: BTreeMap<String, String> = {
        let mut m = ru.env_vars.clone();
        m.extend(unit.unit_env_vars.iter().map(|(k, v)| (k.clone(), v.clone())));
        m
    };
    // R1 (CS-0164): only per-unit exports (chore params) go into the spawned
    // step's process environment. Config `var.*` values (`ru.env_vars`) stay
    // in `env_vars` for `cook.env` / consulted / probe lookup but are
    // `$<NAME>`-only — never injected into a step.
    let process_env_vars: BTreeMap<String, String> = unit
        .unit_env_vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let presatisfied = is_presatisfied(unit);
    WorkNode {
        payload: if presatisfied { None } else { Some(unit.payload.clone()) },
        recipe_name: ru.recipe_name.clone(),
        cache_meta: if presatisfied { None } else { unit.cache_meta.clone() },
        working_dir: ru.working_dir.clone(),
        env_vars: merged_env_vars,
        process_env_vars,
        test_name: unit.test_name.clone(),
        member: unit.member.clone(),
    }
}

/// A unit is presatisfied (cached) when it has an empty shell command and
/// takes no part in caching.
///
/// COOK-360: the second half was written as `cache_meta.is_none()`, which
/// conflated "never cached" with "cached, but with no outputs". Exactly
/// equivalent today — nothing yet declares a cacheable output-less unit — and
/// stated so that when one does, it is not silently swept in here as a no-op.
fn is_presatisfied(unit: &CapturedUnit) -> bool {
    let uncacheable = cook_contracts::cache::record::cacheability(unit.cache_meta.as_ref())
        == cook_contracts::cache::record::Cacheability::Uncacheable;
    match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.is_empty() && uncacheable,
        // CS-0191: this needed a `Test` arm, to keep a Lua-body test — which
        // had an empty `cmd` by construction — from reading as the legacy
        // empty-shell no-op. The payload now says it: a Lua body is a
        // `LuaChunk` and falls through to `false`, and a command test is a
        // `Shell` the arm above judges on its own terms.
        _ => false,
    }
}

/// §22.1.2 terminal-output rule: check that no recipe's literal `inputs[]`
/// path is matched by another recipe's glob `outputs[]` pattern.
///
/// Detection is purely syntactic — no filesystem access, no working-directory
/// resolution. For each recipe that has any glob in its `outputs[]` entries,
/// we compile each glob pattern and test it against every literal `inputs[]`
/// string in every other recipe. On a positive match the function returns an
/// `EngineError::GlobbedOutputCrossRecipeEdge` naming both recipes, the
/// offending input path, and the matching pattern.
///
/// A `requires` edge alone (DAG ordering with no file-content dependency)
/// is NOT affected: this check only fires when a downstream recipe lists a
/// matching path as a literal member of its `inputs[]`.
pub(crate) fn check_globbed_output_cross_recipe_edges(
    recipes: &[RecipeUnits],
) -> Result<(), EngineError> {
    use globset::Glob;

    // Collect terminal output patterns per recipe name. Terminal outputs are
    // glob patterns (has_glob_meta) AND directory outputs (trailing `/`); both
    // are "build-owned" and must not be referenced as literal file inputs by
    // other recipes.  Use BTreeMap for deterministic iteration order so the
    // first error emitted is stable.
    let mut globbed_outputs_by_recipe: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for r in recipes {
        for unit in &r.units {
            if let Some(meta) = &unit.cache_meta {
                for entry in &meta.output_paths {
                    if cook_cache::is_terminal_output(entry) {
                        globbed_outputs_by_recipe
                            .entry(r.recipe_name.as_str())
                            .or_default()
                            .push(entry.as_str());
                    }
                }
            }
        }
    }

    if globbed_outputs_by_recipe.is_empty() {
        return Ok(());
    }

    // Compile each pattern once. Patterns that fail to compile are silently
    // skipped here; downstream, resolve_glob in cook-fingerprint will also
    // treat them as matching no files, so the unit re-runs every invocation
    // without raising a diagnostic. This is a known footgun; a future CS
    // could add a register-time validity check.
    //
    // Asymmetry note: we use `globset` 0.4 here for pure string matching
    // (no filesystem access, per §22.1.2 "MUST NOT consult the filesystem").
    // `globset` 0.4's `**` semantics differ from `glob` 0.3's; in particular
    // `globset` matches `"build/**"` against `"build/foo.o"` correctly
    // without the trailing-`**` normalisation that `resolve_output_paths`
    // applies before passing patterns to `glob` 0.3. Do not add
    // `normalize_glob_pattern` here — it would either be a no-op or weaken
    // matching against legitimate paths.
    //
    // Directory-output normalisation: a trailing-`/` entry such as `"pkg/"`
    // has no glob metacharacter, so `Glob::new("pkg/")` would only match the
    // literal path `"pkg/"` — not files inside the directory. Expand it to
    // `"pkg/**"` so that any literal path under the subtree is matched.  The
    // raw entry string (`"pkg/"`) is kept for the diagnostic message so the
    // user sees the pattern they actually wrote in their Cookfile.
    let matchers: Vec<(&str, Vec<(&str, globset::GlobMatcher)>)> = globbed_outputs_by_recipe
        .iter()
        .map(|(name, patterns)| {
            let built: Vec<(&str, globset::GlobMatcher)> = patterns
                .iter()
                .filter_map(|p| {
                    let pat = if p.ends_with('/') {
                        format!("{p}**")
                    } else {
                        (*p).to_string()
                    };
                    Glob::new(&pat).ok().map(|g| (*p, g.compile_matcher()))
                })
                .collect();
            (*name, built)
        })
        .collect();

    // Test every other recipe's literal inputs[] strings against each pattern.
    for r in recipes {
        for unit in &r.units {
            if let Some(meta) = &unit.cache_meta {
                for entry in &meta.inputs {
                    // Skip entries that are themselves patterns — §22.1.2 only
                    // prohibits literal downstream inputs matching upstream
                    // glob patterns. A downstream pattern input is a different
                    // semantic (expansion at execute time) and is not checked
                    // here. Read off the declaration (§17.1.1.2) rather than
                    // re-scanned for metacharacters, so a real file whose name
                    // contains one is checked as the literal it is.
                    if entry.is_pattern() {
                        continue;
                    }
                    let input_path = &entry.path;
                    for (upstream_name, patterns) in &matchers {
                        // A recipe cannot violate its own terminal-output rule.
                        if *upstream_name == r.recipe_name.as_str() {
                            continue;
                        }
                        for (pat_str, matcher) in patterns {
                            if matcher.is_match(input_path.as_str()) {
                                return Err(EngineError::GlobbedOutputCrossRecipeEdge {
                                    upstream: upstream_name.to_string(),
                                    downstream: r.recipe_name.clone(),
                                    input: input_path.clone(),
                                    pattern: pat_str.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// §16.1.2 read-after-write rule: a literal `outputs[]` entry of recipe `A`
/// equal to a literal `inputs[]` entry of recipe `B`, both in the current
/// build closure, with no ordering path `B → A`, is rejected at plan time.
///
/// **A diagnostic, NOT an inferred edge.** §10.6 is explicit that only name
/// references create edges and that an implementation MUST NOT infer an edge
/// from path-string equality. So this function rejects the plan and
/// synthesises nothing — the author must name the producer.
///
/// # Why the predicate is DIRECTED (and §16.1.1's is not)
///
/// [`detect_output_collisions`] (§16.1.1, write-write) builds an UNDIRECTED
/// recipe graph and passes any connected pair. That is correct there: two
/// recipes writing the same path race, and EITHER ordering serialises the
/// race, so any path suffices.
///
/// Read-after-write is asymmetric. Only `B requires A` — producer first — is
/// correct. The reverse path, `A requires B`, orders the consumer BEFORE the
/// write: that is not an ambiguous race that scheduling might happen to win,
/// but a DETERMINISTIC stale/missing read, strictly worse than the unordered
/// case. An undirected predicate would wave it through as "dep-related,
/// fine". So this function deliberately does NOT reuse `connected()`; it uses
/// [`requires_transitively`] and passes only when `B` transitively requires
/// `A`. Every other configuration, the reverse path included, diagnoses.
///
/// # Why the check is CLOSURE-scoped (and §22.1.2's is not)
///
/// The caller passes the build closure, not the whole workspace. A literal
/// output is not build-owned, so `cook producer && cook consumer` is a
/// legitimate sequential workflow and MUST NOT be rejected — with only
/// `consumer` in the closure there is no producer to compare against and this
/// function stays silent. §22.1.2's glob rule is workspace-wide precisely
/// because terminality there is an OWNERSHIP claim, which holds regardless of
/// what is being built. (`tests/raw_path_cross_recipe_edge.rs` pins the
/// out-of-closure case; a workspace-wide check here would turn it red.)
///
/// Terminal (glob / directory) outputs are [`check_globbed_output_cross_recipe_edges`]'s
/// business and are skipped here so the two rules never double-report.
///
/// # Precedence over §16.1.1
///
/// §16.1.2 states normatively that where a closure violates BOTH this rule and
/// §16.1.1, the §16.1.1 collision is what must be reported: with two unordered
/// writers of one path, the producer this scan names is arbitrary (whichever
/// `BTreeSet` yields first), and the fix it advises would silence the
/// read-after-write while leaving the write-write race intact.
///
/// This function nonetheless runs BEFORE [`detect_output_collisions`] (which
/// opens [`build_dag`]), and that is deliberate, not an oversight. The
/// ordering is unobservable here: [`detect_output_collisions`] is undirected
/// over a graph that INCLUDES the requested target, so every recipe in a
/// single-target closure is connected to the target and therefore to every
/// other member — it cannot return `Some` for any closure `run_inner` builds.
/// There is nothing for this check to mask. Adding a pre-pass to "fix" the
/// order would buy an inert scan. If §16.1.1's predicate is ever narrowed so
/// it can fire on a real closure, the precedence must be pinned at that point.
///
/// # Which surfaces this can actually see
///
/// Detection needs a SOURCE-DECLARED path — one fixed by the Cookfile text,
/// not by what is on disk. `cook.add_unit`'s `inputs[]`/`outputs[]` and `cook`
/// step output literals qualify. An `ingredients` literal does NOT: it is a
/// glob resolved against the filesystem at register time (§21.2.1), so an
/// absent artifact matches zero files and reaches `input_paths` as nothing at
/// all. Covering it would invert the rule — silent on the cold build that
/// actually races, loud only once a stale artifact already exists. §16.1.2's
/// enumeration is therefore closed over the two surfaces above, and Note
/// 16.1.2.2 records the exclusion. (§10.6's *prohibition* still covers
/// `ingredients` literals; that is a rule about what must not happen and
/// needs no detection.)
pub(crate) fn check_literal_read_after_write(
    recipe_units: &[RecipeUnits],
) -> Result<(), EngineError> {
    // canonical path -> recipes declaring it as a LITERAL output.
    let mut producers_by_path: BTreeMap<PathBuf, BTreeSet<&str>> = BTreeMap::new();
    for ru in recipe_units {
        for unit in &ru.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            for output in &meta.output_paths {
                // §22.1.2 owns terminal outputs — do not double-report.
                if cook_cache::is_terminal_output(output) {
                    continue;
                }
                producers_by_path
                    .entry(ru.working_dir.join(output))
                    .or_default()
                    .insert(ru.recipe_name.as_str());
            }
        }
    }

    if producers_by_path.is_empty() {
        return Ok(());
    }

    // DIRECTED recipe graph: name -> the recipes it requires.
    //
    // Direction convention: an entry "A requires B" means B executes BEFORE A
    // (`build_adjacency` sets `deps[name] = info.requires`, and `visit()`
    // recurses into deps before pushing the node). So an edge here points
    // from a recipe to something that runs earlier, and "B transitively
    // requires A" is exactly the ordering this rule demands.
    //
    // Both cross-recipe channels count as ordering: coarse `deps` (the
    // recipe-header colon list, into which the analyzer also folds
    // `cook.require_recipe`) and fine-grained `dep_edges` (`$<sigil>` /
    // `cook.dep_output`). Both are name references, so both are §10.6 edges.
    let mut requires: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for ru in recipe_units {
        let entry = requires.entry(ru.recipe_name.as_str()).or_default();
        for dep in &ru.deps {
            entry.insert(dep.as_str());
        }
        for (_, dep_name) in &ru.dep_edges {
            entry.insert(dep_name.as_str());
        }
    }

    for ru in recipe_units {
        for unit in &ru.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            for entry in &meta.inputs {
                // A pattern input expands at execute time — not a literal
                // path match, and not this rule's business.
                if entry.is_pattern() {
                    continue;
                }
                let input = &entry.path;
                let canonical = ru.working_dir.join(input);
                let Some(producers) = producers_by_path.get(&canonical) else {
                    continue;
                };
                for producer in producers {
                    // A recipe's own units are already ordered within the
                    // recipe; reading your own output is not a race.
                    if *producer == ru.recipe_name.as_str() {
                        continue;
                    }
                    if requires_transitively(&requires, ru.recipe_name.as_str(), producer) {
                        continue;
                    }
                    return Err(EngineError::LiteralReadAfterWrite {
                        producer: (*producer).to_string(),
                        consumer: ru.recipe_name.clone(),
                        path: input.clone(),
                    });
                }
            }
        }
    }

    Ok(())
}

/// BFS reachability over the DIRECTED recipe `requires` graph: does `from`
/// transitively require `to`?
///
/// Deliberately NOT [`connected`] — see [`check_literal_read_after_write`] for
/// why §16.1.2 needs a directed predicate where §16.1.1 wants an undirected
/// one. Do not "unify" the two.
fn requires_transitively(graph: &BTreeMap<&str, BTreeSet<&str>>, from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(from);
    seen.insert(from);
    while let Some(node) = queue.pop_front() {
        if node == to {
            return true;
        }
        if let Some(deps) = graph.get(node) {
            for d in deps {
                if seen.insert(*d) {
                    queue.push_back(*d);
                }
            }
        }
    }
    false
}

/// Detect non-dep-related recipes that declare the same canonical output path.
///
/// Returns `Some(EngineError::OutputCollision)` for the first colliding path
/// found (deterministic — driven by `BTreeMap` iteration order). Returns
/// `None` when the wave is collision-free.
fn detect_output_collisions(recipe_units: &[RecipeUnits]) -> Option<EngineError> {
    // path -> set of recipe names that declare it
    let mut by_path: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for ru in recipe_units {
        for unit in &ru.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            for output in &meta.output_paths {
                let canonical = ru.working_dir.join(output);
                by_path
                    .entry(canonical)
                    .or_default()
                    .insert(ru.recipe_name.clone());
            }
        }
    }

    // Build a recipe-level dep graph from RecipeUnits.deps. Edges are
    // bidirectional for the "dep-related" reachability check, since either
    // direction (A depends on B, or B depends on A) imposes ordering.
    let mut undirected: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ru in recipe_units {
        undirected.entry(ru.recipe_name.clone()).or_default();
        for dep in &ru.deps {
            undirected
                .entry(ru.recipe_name.clone())
                .or_default()
                .insert(dep.clone());
            undirected
                .entry(dep.clone())
                .or_default()
                .insert(ru.recipe_name.clone());
        }
    }

    for (path, recipes) in &by_path {
        if recipes.len() < 2 {
            continue;
        }
        // Pick any two recipes from the colliding set and check whether they
        // are connected in the undirected dep graph. If any pair is
        // disconnected, we have a true collision.
        let names: Vec<&String> = recipes.iter().collect();
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                if !connected(&undirected, names[i], names[j]) {
                    return Some(EngineError::OutputCollision {
                        path: path.clone(),
                        recipes: recipes.iter().cloned().collect(),
                    });
                }
            }
        }
    }

    None
}

/// BFS reachability over the undirected recipe dep graph.
fn connected(graph: &BTreeMap<String, BTreeSet<String>>, a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(a);
    seen.insert(a);
    while let Some(node) = queue.pop_front() {
        if node == b {
            return true;
        }
        if let Some(neighbors) = graph.get(node) {
            for n in neighbors {
                if seen.insert(n.as_str()) {
                    queue.push_back(n.as_str());
                }
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "tests/dag_builder_tests.rs"]
mod tests;

