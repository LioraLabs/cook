//! Workspace-level register pass: invokes `cook_register::register_cookfile`
//! once per Cookfile (root + each import), merges per-import results into a
//! single [`RegisteredWorkspace`] with qualified names, units, probes, and
//! per-Cookfile env / working-dir / alias-dirs entries.
//!
//! This is the pipeline-layer entry point that replaces today's
//! `build_*_registries` helpers (SHI-222 CS-0077 Phase 5 Task 5.1). The CLI
//! commands call [`register_workspace`] with a [`RegisterMode`] and then
//! hand the resulting `RegisteredWorkspace` to `cook_engine::run::run`.
//!
//! One entry point: [`register_workspace`].
//! Iterates the root + every import in `Workspace::imports`, calling
//! `register_cookfile` on each, then merges the per-import results. A
//! single-Cookfile project (no imports) is simply a workspace of one
//! member — its root has no `import` declarations, so `Workspace::imports`
//! is empty and the root registers under the empty qualified prefix `""`,
//! same as any other workspace's root.
//!
//! The merge logic prefixes each registered name, unit key, and probe key
//! with the import's qualified prefix (`""` for root). Per-Cookfile
//! `final_env`, `working_dir`, and `alias_dirs` are recorded under the same
//! prefix key.
//!
//! `cache_ctx` is threaded through to each `register_cookfile` call so that
//! probes registered during the register pass see real machine identity and
//! the env denylist (CS-0074). The CLI lifts the `CacheContext` construction
//! out of `run_inner` in Task 5.3 so the register pass observes the same
//! context the executor will later use.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cook_register::{register_cookfile, RegisterSessionBuilder, SharedMemberOutputs, SharedTerminalOutputs};

use super::env::parse_cli_overrides;
use super::error::PipelineError;
use super::recipe_info::find_full_prefix;
use super::workspace::{LoadedCookfile, Workspace};
use crate::registered_workspace::RegisteredWorkspace;

/// How the register pass binds a CLI dispatch target. The register layer has
/// three distinct target behaviors (see `cook-register/src/engine.rs`:
/// `target_recipe` / `reachable_from_target`), and this enum names them so
/// callers cannot conflate the modes.
#[derive(Debug, Clone, Copy)]
pub enum RegisterMode<'a> {
    /// Dispatch to the named recipe / chore, binding `argv` to its chore
    /// parameters (COOK-36 Task 4). `name` is the fully-qualified target, so
    /// the binding lands on whichever member OWNS it: the root binds the name
    /// verbatim, and an import binds the name with its qualified prefix
    /// stripped, since a member's bodies are registered under local names.
    ///
    /// This used to bind only on the root, on the premise that chores are
    /// always defined in and dispatched from the root. They are not, and a
    /// parametric chore in an import was silently skipped as a result
    /// (COOK-349).
    Dispatch { name: &'a str, argv: &'a [String] },
    /// Register with a target that matches nothing: the register pass behaves
    /// as targeted (the member-source pre-pass and parametric chore bodies
    /// are pruned to the — empty — target-reachable set) but no body receives
    /// argv. Used by read-only introspection such as `cook affected`.
    Introspect,
    /// No dispatch target at all: chore bodies are invoked normally for
    /// enumeration (listing, DAG assembly). This is a different register-time
    /// behavior than [`RegisterMode::Introspect`].
    Enumerate,
}

/// Order the workspace's Cookfiles for the register pass so that every
/// cross-Cookfile `cook.dep_output("alias.recipe")` sees its producer already
/// registered: **importees before importers, with the root last**.
///
/// A recipe's terminal outputs are written into the shared map only *after* its
/// body runs (cook-register populates them from `last_cook_step_outputs`), and
/// a consumer body reads that same map at register time. So a consumer Cookfile
/// must be registered after every Cookfile it references. This is a post-order
/// DFS over the import DAG (acyclic — §11.5 rejects import cycles): post-order
/// emits a node only after all its importees, and the root (which imports but is
/// imported by no one) emits last. Returns canonical directory paths.
fn cookfile_registration_order(workspace: &Workspace) -> Vec<PathBuf> {
    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    // Import-DAG adjacency: importer dir -> [importee dir, ...] (deduped,
    // declaration order preserved).
    let mut adj: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for (parent, _alias, target) in &workspace.namespace_map {
        let entry = adj.entry(canon(parent)).or_default();
        let t = canon(target);
        if !entry.contains(&t) {
            entry.push(t);
        }
    }

    fn visit(
        node: PathBuf,
        adj: &BTreeMap<PathBuf, Vec<PathBuf>>,
        visited: &mut BTreeSet<PathBuf>,
        order: &mut Vec<PathBuf>,
    ) {
        if !visited.insert(node.clone()) {
            return;
        }
        if let Some(children) = adj.get(&node) {
            for child in children {
                visit(child.clone(), adj, visited, order);
            }
        }
        order.push(node);
    }

    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut order: Vec<PathBuf> = Vec::new();
    visit(canon(&workspace.root.dir), &adj, &mut visited, &mut order);

    // Safety net: register any import not reachable from root (should not occur,
    // since the workspace is built by walking imports outward from root).
    for path in workspace.imports.keys() {
        let c = canon(path);
        if visited.insert(c.clone()) {
            order.push(c);
        }
    }

    order
}

/// Workspace-root-relative, forward-slashed label of a member's Cookfile
/// (§20.2.3): the same member yields the same label whether it registers as
/// the entry Cookfile or as an import, so cache identity cannot depend on
/// the invocation directory. Falls back to the bare "Cookfile" when the
/// member does not sit under the workspace root (defensive; workspace load
/// enforces containment).
fn root_anchored_cookfile_label(workspace_root: &Path, member_dir: &Path) -> String {
    // `Workspace::load` canonicalizes `workspace_root`, but manually
    // constructed workspaces (tests) may pass a non-canonical root —
    // canonicalize both sides so strip_prefix compares like with like.
    let root = std::fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let canon = std::fs::canonicalize(member_dir).unwrap_or_else(|_| member_dir.to_path_buf());
    match canon.join("Cookfile").strip_prefix(&root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => "Cookfile".to_string(),
    }
}

/// Build the base [`RegisterSessionBuilder`] for one workspace member: the
/// per-member env policy plus the CLI-override / selected-config / qualified-
/// prefix scaffold every register-layer pass shares.
///
/// Var policy (CS-0172): every member starts with an EMPTY `var` namespace.
/// A member's own `config` blocks are its only source of declared variables —
/// per §11.6 a config block's scope is exactly its own Cookfile — and the CLI
/// `--set` overrides apply on top, checked against the declared set. There is
/// no ambient-process-env layer and no `.env` layer for `is_root` to select.
/// This is the ONE place that policy lives; [`register_workspace`],
/// [`list_workspace_names`], and [`codegen_with_module_recipes`] all derive
/// their per-member builders from here.
fn member_base_builder(
    member: &LoadedCookfile,
    prefix: &str,
    is_root: bool,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<RegisterSessionBuilder, PipelineError> {
    let _ = is_root;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    Ok(RegisterSessionBuilder::new(member.dir.clone(), HashMap::new())
        .with_cli_overrides(cli_overrides)
        .with_selected_config(config.map(|s| s.to_string()))
        .with_qualified_prefix(prefix.to_string()))
}

/// Workspace members in root-first order: `(member, canonical_dir, prefix,
/// is_root)` for the root (prefix `""`) followed by every import in
/// canonical-path order with its workspace qualified prefix.
///
/// This is the iteration order of the per-member-independent passes
/// ([`list_workspace_names`], [`codegen_with_module_recipes`]) — it determines
/// `cook list` output order, so it stays root-first. The register pass orders
/// members importees-first / root-last instead (see
/// [`cookfile_registration_order`]) because cross-Cookfile terminal-output
/// lookups need producers registered before consumers.
fn members_root_first(
    workspace: &Workspace,
) -> Vec<(&LoadedCookfile, PathBuf, String, bool)> {
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    let mut out = vec![(&workspace.root, root_canon, String::new(), true)];
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        out.push((loaded, canonical_path.clone(), prefix, false));
    }
    out
}

/// Run the register pass once per Cookfile in `workspace` (root + every
/// import in `Workspace::imports`) and merge the per-import results.
///
/// Names, unit keys, and probe keys are qualified with the import's prefix
/// (`""` for root). Per-Cookfile `final_env`, `working_dir`, and `alias_dirs`
/// are recorded under that same prefix key in the returned
/// [`RegisteredWorkspace`].
///
/// `mode` selects how the CLI dispatch target binds (see [`RegisterMode`]);
/// the target-argv binding applies only to the root Cookfile's builder.
///
/// A single [`SharedTerminalOutputs`] is threaded through every per-Cookfile
/// builder so cross-Cookfile `cook.dep_output("alias.recipe")` lookups
/// resolve through the same backing storage. Each builder also receives the
/// canonical qualified prefix for the importer's local aliases via
/// `with_alias_qualified_prefixes`, so diamond-import targets resolve to
/// their one canonical storage key regardless of which chain reached them.
///
/// `cache_ctx` is cloned into each per-Cookfile `register_cookfile` call.
/// Task 5.3 lifts the cache_ctx construction out of `run_inner` so the
/// register pass and the executor observe the same context.
pub fn register_workspace(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
    mode: RegisterMode<'_>,
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
    // Backend for the `ingredients <probe>` pre-pass only — see
    // `register_cookfile`'s parameter of the same name (COOK-359).
) -> Result<RegisteredWorkspace, PipelineError> {
    let shared_outputs: SharedTerminalOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    let shared_member_outputs: SharedMemberOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));

    let mut ws = RegisteredWorkspace {
        warnings: Vec::new(),
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
        terminal_outputs: BTreeMap::new(),
    };

    // Register Cookfiles importees-first / root-last so every cross-Cookfile
    // `cook.dep_output` call sees its producer's terminal outputs already
    // populated in the shared map (see `cookfile_registration_order`).
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    for dir in cookfile_registration_order(workspace) {
        let is_root = dir == root_canon;
        let (member, prefix): (&LoadedCookfile, String) = if is_root {
            // Root Cookfile: empty qualified prefix.
            (&workspace.root, String::new())
        } else if let Some(loaded) = workspace.imports.get(&dir) {
            // Imports: canonical workspace qualified prefix (computed from
            // the namespace map).
            (loaded, find_full_prefix(workspace, &dir))
        } else {
            continue;
        };

        let alias_dirs = workspace.alias_dirs_for(&member.dir);
        let alias_qp = workspace.alias_qualified_prefixes_for(&member.dir);
        let mut builder =
            member_base_builder(member, &prefix, is_root, config, env_overrides)?
                .with_shared_terminal_outputs(shared_outputs.clone())
                .with_workspace_root(workspace.workspace_root.clone())
                .with_shared_member_outputs(shared_member_outputs.clone())
                .with_alias_dirs(alias_dirs.clone())
                .with_alias_qualified_prefixes(alias_qp.clone())
                .with_cookfile_label(root_anchored_cookfile_label(
                    &workspace.workspace_root,
                    &member.dir,
                ));
        // Bind the dispatch target to whichever member OWNS the targeted name.
        //
        // This used to bind only on the root Cookfile, on the premise that
        // "chores are always defined in and dispatched from root". They are
        // not: a chore may be declared in an imported member and dispatched by
        // its qualified name (`cook standard.against-tag 0.18`).
        //
        // A member's register pass therefore saw `target_recipe: None`, so
        // `is_target` was false for every name in it. A PARAMLESS chore
        // survived that — engine.rs invokes it unconditionally — but a
        // PARAMETRIC one fell through to the skip arm, whose whole job is to
        // avoid calling a body that would nil-index `__cook_params`. The result
        // was a chore that registered zero units, reported `0 nodes`, exited 0,
        // and never ran its body (COOK-349). Silent and success-coded.
        //
        // `prefix` carries no trailing dot (qualification is `{prefix}.{name}`),
        // so a member owns the target when the name starts with `{prefix}.`;
        // what the member's own pass must see is the LOCAL name, since that is
        // what its bodies are registered under. Matching the canonical prefix
        // alone is complete: `merge_into` qualifies every registered name with
        // exactly this prefix, and uses alias prefixes only to rewrite
        // cross-Cookfile `requires`, never to register an alias name.
        builder = match mode {
            RegisterMode::Dispatch { name, argv } => {
                if is_root {
                    builder.with_target_argv(name.to_string(), argv.to_vec())
                } else if let Some(local) = name
                    .strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .filter(|local| !local.is_empty())
                {
                    builder.with_target_argv(local.to_string(), argv.to_vec())
                } else {
                    builder
                }
            }
            RegisterMode::Introspect if is_root => {
                builder.with_target_argv(String::new(), Vec::new())
            }
            RegisterMode::Introspect | RegisterMode::Enumerate => builder,
        };

        let registered =
            register_cookfile(
                builder,
                &member.lua_source,
                cache_ctx.clone(),
            )
                .map_err(map_register_error)?;
        merge_into(&mut ws, &prefix, &alias_qp, registered);
        ws.working_dir_by_prefix
            .insert(prefix.clone(), member.dir.clone());
        ws.alias_dirs_by_prefix.insert(prefix, alias_dirs);
    }

    // Snapshot the now-fully-populated terminal-outputs map so the
    // execute-phase worker VMs can resolve cook.dep_output / dep_output_list
    // (§24.7). Every producer's outputs are recorded during the register pass
    // above; the map is not written again before execute phase.
    ws.terminal_outputs = shared_outputs
        .lock()
        .expect("terminal_outputs mutex poisoned")
        .clone();

    reject_duplicate_outputs(&ws)?;

    Ok(ws)
}

/// Reject a graph in which two work units declare the same `output`.
///
/// Such a graph has no correct execution: the two units race to write one
/// path, the loser is seen as drifted on the next run and rebuilt, and it
/// hands the loss straight back — a build that reports success and never
/// settles, forever, with no diagnostic. It is not a hypothetical: `cook_cc`
/// derived object paths from the source basename until CS-0167, so every tree
/// with repeated source filenames produced exactly this, and the symptoms
/// (permanent cache churn, a 128 MB index reserialised on a no-op run, wrong
/// header dependency sets, hundreds of undefined symbols at link) each cost
/// real debugging time before the shared cause was found.
///
/// Checked across the whole run rather than per recipe: two *recipes* writing
/// one path is the same defect wearing a different hat, and the per-recipe
/// check would miss it.
///
/// Scope is deliberately **literal output paths only**. An output glob
/// (`dist/**`) and a build-owned directory output (§17.6, CS-0119) are claims
/// over a *set*, not over a path, and what overlapping claims should mean is
/// governed by output reconciliation rather than by unit identity. Two units
/// declaring `dist/**` may well be a defect too, but it is a different
/// question with different semantics, and deciding it here would silently
/// redefine those. Deferred, not overlooked.
fn reject_duplicate_outputs(ws: &RegisteredWorkspace) -> Result<(), PipelineError> {
    // output path -> (recipe, short description of the producing unit)
    let mut seen: BTreeMap<&str, (&str, String)> = BTreeMap::new();

    for (recipe_name, units) in &ws.units_by_recipe {
        for unit in &units.units {
            for out in &unit.output_paths {
                if is_output_pattern(out) || cook_cache::is_dir_output(out) {
                    continue;
                }
                if let Some((first_recipe, first_unit)) = seen.get(out.as_str()) {
                    return Err(PipelineError::DuplicateOutput {
                        output: out.clone(),
                        first_recipe: (*first_recipe).to_string(),
                        first_unit: first_unit.clone(),
                        second_recipe: recipe_name.clone(),
                        second_unit: describe_unit(unit),
                    });
                }
                seen.insert(out.as_str(), (recipe_name.as_str(), describe_unit(unit)));
            }
        }
    }
    Ok(())
}

/// True when an output declaration is a glob pattern rather than a literal
/// path — see the scope note on [`reject_duplicate_outputs`].
fn is_output_pattern(out: &str) -> bool {
    out.contains('*') || out.contains('?') || out.contains('[')
}

/// One-line rendering of a unit for the duplicate-output diagnostic: the
/// command text is what an author recognises, so lead with it.
fn describe_unit(unit: &cook_contracts::CapturedUnit) -> String {
    use cook_contracts::WorkPayload;
    let raw = match &unit.payload {
        WorkPayload::Shell { cmd, .. } | WorkPayload::Interactive { cmd, .. } => cmd.clone(),
        WorkPayload::LuaChunk { .. } => "<lua step>".to_string(),
        _ => "<step>".to_string(),
    };
    // Shell payloads carry a `set -e` preamble the author never wrote; drop it
    // and flatten to one line so the two sites line up readably.
    let body = raw
        .trim()
        .strip_prefix("set -e")
        .unwrap_or_else(|| raw.trim())
        .trim();
    let flattened = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let mut short: String = flattened.chars().take(120).collect();
    if flattened.chars().count() > 120 {
        short.push('…');
    }
    short
}

/// Run [`cook_register::list_names`] for every Cookfile in `workspace`
/// (root + every import) and return each discovered recipe with its
/// registration metadata intact.
///
/// Workspace-level counterpart to [`register_workspace`]: each import's
/// names are prefixed with its qualified workspace prefix (the `name`
/// field is rewritten in place; every other field is passed through). This
/// avoids invoking any recipe body and avoids firing probe queries — it's
/// the cheap path used by `cook list` / `cook menu`.
///
/// Returns the full [`cook_register::RegisteredRecipePub`] rather than a
/// name/kind projection: `cook list` renders the `origin` annotation a
/// module attaches to a recipe it mints, so the listing path must not
/// discard per-recipe metadata on the way out.
pub fn list_workspace_names(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<Vec<cook_register::RegisteredRecipePub>, PipelineError> {
    let mut out: Vec<cook_register::RegisteredRecipePub> = Vec::new();
    for (member, _canon, prefix, is_root) in members_root_first(workspace) {
        let builder = member_base_builder(member, &prefix, is_root, config, env_overrides)?;
        let names = cook_register::list_names(builder, &member.lua_source)
            .map_err(map_register_error)?;
        for mut n in names {
            n.name = if is_root {
                n.name
            } else {
                format!("{prefix}.{}", n.name)
            };
            out.push(n);
        }
    }
    Ok(out)
}

/// Re-run codegen for every Cookfile in the workspace against the *full*
/// register-phase recipe set (§10.2 step 2, CS-0094), generalising
/// the former single-Cookfile-only pass to every workspace member.
///
/// The load-time codegen passes classify `$<NAME>` placeholders using only
/// statically parsed `recipe` blocks plus the §7.3 alias union. A `$<NAME>`
/// naming a recipe registered at register-phase by a top-level module call
/// (e.g. `cook_cc.bin("x")`) is invisible to those passes and mis-lowers to
/// `cook.require_var(...)`, hard-erroring when the body runs during the
/// register pass. This runs the cheap body-free [`cook_register::list_names`]
/// pass per member (same env policy as [`list_workspace_names`]), unions the
/// discovered names with the static set — locally, and as `alias.name` on
/// each importer — and regenerates every member's Lua in place. Feeding the
/// static-name Lua to `list_names` is safe: it never invokes a recipe body,
/// so the latent mis-lowering is never reached during discovery.
pub fn codegen_with_module_recipes(
    workspace: &mut Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<(), PipelineError> {
    let mut discovered: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for (member, canon, prefix, is_root) in members_root_first(workspace) {
        let builder = member_base_builder(member, &prefix, is_root, config, env_overrides)?;
        let names = cook_register::list_names(builder, &member.lua_source)
            .map_err(map_register_error)?;
        discovered.insert(canon, names.into_iter().map(|n| n.name).collect());
    }
    super::workspace::regenerate_lua_sources(workspace, &discovered)
}

/// Merge a per-Cookfile [`cook_register::RegisteredCookfile`] into the
/// workspace-level [`RegisteredWorkspace`], qualifying every recipe name,
/// unit key, probe key, and intra-Cookfile `requires` entry with `prefix`
/// (empty for the root).
///
/// Intra-Cookfile `requires` entries (e.g. `recipe wasm: generate` inside
/// `tree-sitter-cook/Cookfile` imported as `ts`) must be rewritten from the
/// local name `generate` to the qualified `ts.generate` so the analyzer's
/// dep-graph walk sees a consistent fully-qualified namespace. Without this
/// qualification `analyzer::build_adjacency` walks every recipe in the
/// workspace and errors `UnknownRecipe("generate")` even when the target
/// closure (e.g. `cook package`) does not transitively touch the import.
fn merge_into(
    ws: &mut RegisteredWorkspace,
    prefix: &str,
    alias_qualified_prefixes: &BTreeMap<String, String>,
    rc: cook_register::RegisteredCookfile,
) {
    ws.warnings.extend(rc.warnings.iter().cloned());
    let qualify = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    // Local recipe names registered by this Cookfile — used to distinguish
    // intra-Cookfile dep references (`requires=["generate"]` resolving inside
    // `tree-sitter-cook/Cookfile`) from already-qualified cross-Cookfile
    // references that callers may have produced explicitly. Intra-Cookfile
    // requires get the prefix; cross-Cookfile or already-qualified ones pass
    // through untouched.
    let local_names: std::collections::BTreeSet<String> =
        rc.names.iter().map(|n| n.name.clone()).collect();
    // Resolve one dep name to its workspace-global key. Shared by the `names`
    // requires-rewrite and the `units_by_recipe` deps-rewrite below so the two
    // views cannot disagree about what a dep name means (COOK-352).
    let qualify_dep = |req: &String| -> String {
        // Cross-Cookfile `alias.recipe` requires → the importee's canonical
        // global key (mirrors `resolve_global_key` and the inferred-deps
        // analyzer). Without this the analyzer sees the local alias name (e.g.
        // `proto.proto_lib`) and errors `UnknownRecipe` when the canonical key
        // is, say, `server.queue.proto.proto_lib` (a diamond / transitive
        // importee whose prefix differs from the local alias).
        if let Some((alias, sub)) = req.split_once('.') {
            if let Some(importee_prefix) = alias_qualified_prefixes.get(alias) {
                return if importee_prefix.is_empty() {
                    sub.to_string()
                } else {
                    format!("{importee_prefix}.{sub}")
                };
            }
        }
        // Intra-Cookfile local name → prefix it with this Cookfile's qualified
        // prefix. Anything else (already-global, or unknown — rejected
        // downstream) passes through untouched.
        if local_names.contains(req) {
            qualify(req)
        } else {
            req.clone()
        }
    };
    for n in rc.names {
        let mut qn = n.clone();
        qn.name = qualify(&n.name);
        qn.requires = n.requires.iter().map(&qualify_dep).collect();
        ws.names.push(qn);
    }
    for (name, mut units) in rc.units_by_recipe {
        let qualified = qualify(&name);
        // Qualify `deps` on the same footing as `recipe_name` below. These are
        // header dep-list entries, recorded by the register layer under LOCAL
        // names; every consumer downstream compares them against qualified
        // names. `run.rs` intersects them with the qualified closure `edges` to
        // rebuild the coarse barrier set, and an unqualified-vs-qualified
        // intersection is empty — so `RecipeUnits::deps` arrived at the engine
        // EMPTY for every recipe in every workspace, root included.
        //
        // The visible symptom was §16.1.2 rejecting a legitimate build and
        // telling the author their recipe "does not require" a producer named
        // in its own header, with a hint to add the dep that was already there.
        // The check could only ever be satisfied through `dep_edges`, i.e. by a
        // `$<producer>` body reference (COOK-352).
        units.deps = units.deps.iter().map(&qualify_dep).collect();
        // Restamp the value's `recipe_name` with the workspace-qualified key
        // so the two never disagree. Everything downstream of the merged map
        // — `WorkNode.recipe_name`, the executor's / `cook why`'s per-recipe
        // cache-manager lookup, recipe trackers, `dag_builder`'s
        // `recipe_leaves` wiring against qualified `deps` / `dep_edges` —
        // keys by the qualified name. Cache identity is unaffected: the
        // local StepEntry index name and the shared-cache namespace derive
        // from `CacheMeta.recipe_name`, which stays Cookfile-local
        // (§20.2.3).
        units.recipe_name = qualified.clone();
        ws.units_by_recipe.insert(qualified, units);
    }
    for (key, probe) in rc.probes {
        ws.probes.insert(
            if prefix.is_empty() {
                key
            } else {
                format!("{prefix}.{key}")
            },
            probe,
        );
    }
}

/// Map a [`cook_register::RegisterError`] from one of the helpers in this
/// module onto a [`PipelineError`]. The collision variant is preserved as a
/// structured `PipelineError::RecipeCollision { name, sites }` so the CLI can
/// render the multi-line per-site diagnostic at emit time (SHI-222 Phase 5
/// Task 5.6, spec §8); all other variants fall through to
/// `PipelineError::Other` carrying `RegisterError`'s own `Display` impl —
/// matching the pre-Task-5.6 behavior for non-collision errors.
fn map_register_error(e: cook_register::RegisterError) -> PipelineError {
    match e {
        cook_register::RegisterError::RecipeCollision { name, sites } => {
            PipelineError::RecipeCollision { name, sites }
        }
        // COOK-36 Task 9: append a migration hint when a paramless chore
        // receives exactly one bare-ident-shaped positional — the user likely
        // meant to select a config preset with the old positional form.
        cook_register::RegisterError::ChoreTooManyArgv {
            ref chore,
            declared,
            supplied,
            ref first_unmatched,
        } if declared == 0
            && supplied == 1
            && !first_unmatched.is_empty()
            && first_unmatched
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') =>
        {
            let base = e.to_string();
            PipelineError::Other(format!(
                "{base}. Did you mean a config preset? \
                 Use 'cook {chore} @{first_unmatched}' or \
                 'cook {chore} --config {first_unmatched}'."
            ))
        }
        other => PipelineError::Other(other.to_string()),
    }
}

#[cfg(test)]
#[path = "tests/registers_tests.rs"]
mod tests;
