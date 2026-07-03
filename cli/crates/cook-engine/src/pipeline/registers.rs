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

use super::env::{load_env, parse_cli_overrides, resolve_env};
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
    /// parameters (COOK-36 Task 4). The target binds only to the root
    /// Cookfile's builder — chores are always defined in and dispatched from
    /// the root; import Cookfiles register without argv.
    Dispatch { name: &'a str, argv: &'a [String] },
    /// Register with a target that matches nothing: the register pass behaves
    /// as targeted (the `for_each` probe pre-pass and parametric chore bodies
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
/// Env policy: the root Cookfile gets `.env` layering from its own directory;
/// imports do not inherit it — each sub-Cookfile starts from a fresh env
/// baseline (system env and CLI `--set` overrides still apply). This is the
/// ONE place that policy lives; [`register_workspace`],
/// [`list_workspace_names`], and [`codegen_with_module_recipes`] all derive
/// their per-member builders from here.
fn member_base_builder(
    member: &LoadedCookfile,
    prefix: &str,
    is_root: bool,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<RegisterSessionBuilder, PipelineError> {
    let dotenv_vars = if is_root {
        load_env(&member.dir)
    } else {
        HashMap::new()
    };
    let env = resolve_env(config, dotenv_vars, env_overrides)?;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    Ok(RegisterSessionBuilder::new(member.dir.clone(), env)
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
) -> Result<RegisteredWorkspace, PipelineError> {
    let shared_outputs: SharedTerminalOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    let shared_member_outputs: SharedMemberOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));

    let mut ws = RegisteredWorkspace {
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
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
                .with_shared_member_outputs(shared_member_outputs.clone())
                .with_alias_dirs(alias_dirs.clone())
                .with_alias_qualified_prefixes(alias_qp.clone())
                .with_cookfile_label(root_anchored_cookfile_label(
                    &workspace.workspace_root,
                    &member.dir,
                ));
        if is_root {
            // The dispatch target binds only to the root Cookfile — chores
            // are always defined in and dispatched from root.
            builder = match mode {
                RegisterMode::Dispatch { name, argv } => {
                    builder.with_target_argv(name.to_string(), argv.to_vec())
                }
                RegisterMode::Introspect => {
                    builder.with_target_argv(String::new(), Vec::new())
                }
                RegisterMode::Enumerate => builder,
            };
        }

        let registered =
            register_cookfile(builder, &member.lua_source, cache_ctx.clone())
                .map_err(map_register_error)?;
        merge_into(&mut ws, &prefix, &alias_qp, registered);
        ws.working_dir_by_prefix
            .insert(prefix.clone(), member.dir.clone());
        ws.alias_dirs_by_prefix.insert(prefix, alias_dirs);
    }

    Ok(ws)
}

/// Run [`cook_register::list_names`] for every Cookfile in `workspace`
/// (root + every import) and return the qualified name set with kinds.
///
/// Workspace-level counterpart to [`register_workspace`]: each import's
/// names are prefixed with its qualified workspace prefix. This avoids
/// invoking any recipe body and avoids firing probe queries — it's the
/// cheap path used by `cook list` / `cook menu`.
pub fn list_workspace_names(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<Vec<(String, cook_register::RecipeKind)>, PipelineError> {
    let mut out: Vec<(String, cook_register::RecipeKind)> = Vec::new();
    for (member, _canon, prefix, is_root) in members_root_first(workspace) {
        let builder = member_base_builder(member, &prefix, is_root, config, env_overrides)?;
        let names = cook_register::list_names(builder, &member.lua_source)
            .map_err(map_register_error)?;
        for n in names {
            let qualified = if is_root {
                n.name
            } else {
                format!("{prefix}.{}", n.name)
            };
            out.push((qualified, n.kind));
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
/// `cook.require_env(...)`, hard-erroring when the body runs during the
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
    for n in rc.names {
        let mut qn = n.clone();
        qn.name = qualify(&n.name);
        qn.requires = n
            .requires
            .iter()
            .map(|req| {
                // Cross-Cookfile `alias.recipe` requires → the importee's
                // canonical global key (mirrors `resolve_global_key` and the
                // inferred-deps analyzer). Without this the analyzer sees the
                // local alias name (e.g. `proto.proto_lib`) and errors
                // `UnknownRecipe` when the canonical key is, say,
                // `server.queue.proto.proto_lib` (a diamond / transitive
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
                // Intra-Cookfile local name → prefix it with this Cookfile's
                // qualified prefix. Anything else (already-global, or unknown —
                // rejected downstream) passes through untouched.
                if local_names.contains(req) {
                    qualify(req)
                } else {
                    req.clone()
                }
            })
            .collect();
        ws.names.push(qn);
    }
    for (name, mut units) in rc.units_by_recipe {
        let qualified = qualify(&name);
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
    ws.final_env_by_cookfile
        .insert(prefix.to_string(), rc.final_env);
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
mod tests {
    use super::*;

    /// Build a workspace-of-one directly (no files besides the tempdir) so
    /// the error-mapping tests exercise register_workspace — the sole
    /// registration path after the dual-path collapse.
    fn workspace_of_one(dir: &Path, lua_source: &str) -> Workspace {
        Workspace {
            root: LoadedCookfile {
                // Intentionally inert placeholder AST: registration consumes
                // only `lua_source`; the parsed Cookfile is never re-lowered.
                cookfile: cook_lang::parse("recipe placeholder\n    echo hi\n")
                    .expect("placeholder Cookfile parses"),
                lua_source: lua_source.to_string(),
                dir: dir.to_path_buf(),
            },
            imports: BTreeMap::new(),
            namespace_map: Vec::new(),
            workspace_root: dir.to_path_buf(),
        }
    }

    /// SHI-222 Phase 5 Task 5.6: `register_workspace` must surface
    /// `RegisterError::RecipeCollision` as a structured
    /// `PipelineError::RecipeCollision { name, sites }` (not as
    /// `PipelineError::Other`), so the CLI can render the multi-line
    /// per-site diagnostic at emit time (spec §8) and exit with code 3.
    #[test]
    fn register_workspace_maps_collision_to_typed_variant() {
        let lua_src = r#"
            cook.recipe("build", {requires = {}}, function() end)
            cook.recipe("build", {requires = {}}, function() end)
        "#;
        let tmpdir = tempfile::TempDir::new().unwrap();
        let ws = workspace_of_one(tmpdir.path(), lua_src);
        let result = register_workspace(&ws, None, &[], RegisterMode::Enumerate, None);

        match result {
            Ok(_) => panic!("expected PipelineError::RecipeCollision, got Ok"),
            Err(PipelineError::RecipeCollision { name, sites }) => {
                assert_eq!(name, "build");
                assert_eq!(sites.len(), 2, "both register-phase sites are captured");
                // Both are dynamic `cook.recipe(...)` calls — confirms the
                // typed mapping passes the kind through faithfully.
                for s in &sites {
                    assert_eq!(s.kind, cook_register::RegistrationSiteKind::Dynamic);
                }
            }
            Err(other) => panic!("expected PipelineError::RecipeCollision, got {other:?}"),
        }
    }

    /// `RegisterError` variants other than `RecipeCollision` continue to fall
    /// through to `PipelineError::Other` (pre-Task-5.6 behavior preserved).
    /// Exercises the fallthrough arm of `map_register_error` via a Lua-level
    /// error in the cookfile source.
    #[test]
    fn register_workspace_maps_non_collision_to_other() {
        // Top-level Lua error (undefined function) → RegisterError::Lua →
        // PipelineError::Other.
        let lua_src = "this_function_does_not_exist()\n";
        let tmpdir = tempfile::TempDir::new().unwrap();
        let ws = workspace_of_one(tmpdir.path(), lua_src);
        let result = register_workspace(&ws, None, &[], RegisterMode::Enumerate, None);

        match result {
            Ok(_) => panic!("expected PipelineError::Other, got Ok"),
            Err(PipelineError::Other(_)) => {}
            Err(other) => panic!("expected PipelineError::Other, got {other:?}"),
        }
    }

    /// Workspace-of-one discovery — a recipe registered at
    /// register-phase (invisible to static codegen) must be folded into the
    /// $<NAME> classification set when the workspace path re-codegens.
    #[test]
    fn codegen_with_module_recipes_discovers_dynamic_recipe_workspace_of_one() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cookfile"),
            "recipe consume\n    cook \"build/out\" { cat $<gen> > $<out> }\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
        // Static codegen cannot see `gen` → mis-lowers to require_env.
        assert!(ws.root.lua_source.contains("cook.require_env(\"gen\")"));
        // Simulate a module-registered recipe: append a dynamic registration
        // to the discovery Lua (list_names sees it; bodies never run).
        ws.root.lua_source.push_str(
            "\ncook.recipe(\"gen\", {requires = {}}, function() end)\n",
        );
        codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
        assert!(
            ws.root.lua_source.contains("cook.dep_output(\"gen\")"),
            "expected $<gen> re-lowered to dep_output, got:\n{}",
            ws.root.lua_source
        );
    }

    /// The discovery pass must also cover IMPORTED members —
    /// an importer's `$<alias.recipe>` where `recipe` is module-registered in
    /// the importee must re-lower to dep_output on the workspace path.
    #[test]
    fn codegen_with_module_recipes_discovers_dynamic_recipe_in_importee() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("lib")).unwrap();
        std::fs::write(
            dir.path().join("lib/Cookfile"),
            "recipe lib_static\n    cook \"lib.o\" { echo $<out> }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cookfile"),
            "import lib ./lib\nrecipe top\n    cook \"build/top\" { cat $<lib.gen> > $<out> }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".cookroot"), "").unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
        assert!(ws.root.lua_source.contains("cook.require_env(\"lib.gen\")"));
        // Simulate a module-registered recipe in the importee.
        let lib_canon = std::fs::canonicalize(dir.path().join("lib")).unwrap();
        ws.imports
            .get_mut(&lib_canon)
            .unwrap()
            .lua_source
            .push_str("\ncook.recipe(\"gen\", {requires = {}}, function() end)\n");
        codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
        assert!(
            ws.root.lua_source.contains("cook.dep_output(\"lib.gen\")"),
            "expected $<lib.gen> re-lowered to dep_output, got:\n{}",
            ws.root.lua_source
        );
    }

    /// §20.2.3 cache-identity invariance: the same member Cookfile must
    /// register its units with IDENTICAL `CacheMeta.cookfile_path` and
    /// `CacheMeta.recipe_name` whether it is reached as an import of the
    /// enclosing workspace root (entry = root/Cookfile, registered under
    /// prefix "rust") or as the entry Cookfile itself (workspace-of-one
    /// root, prefix ""). The invocation directory must not influence the
    /// cache namespace.
    #[test]
    fn cache_meta_is_invocation_independent_across_entry_points() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cookfile"),
            "import rust apps/rust\n\nrecipe check\n    echo hi\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("apps/rust")).unwrap();
        std::fs::write(
            dir.path().join("apps/rust/Cookfile"),
            concat!(
                "recipe build\n",
                "    >>{\n",
                "        cook.add_unit({\n",
                "            inputs  = { },\n",
                "            outputs = { \"build/out.txt\" },\n",
                "            command = \"mkdir -p build && echo hi > build/out.txt\",\n",
                "        })\n",
                "    }\n",
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join(".cookroot"), "").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();

        // (i) Entry = the workspace root Cookfile; member registers as an
        //     import under prefix "rust".
        let ws_root = Workspace::load(&root.join("Cookfile"), &root, &[]).unwrap();
        let reg_root = register_workspace(&ws_root, None, &[], RegisterMode::Enumerate, None).unwrap();

        // (ii) Entry = the member Cookfile itself (invoked inside apps/rust);
        //      it registers as the workspace-of-one root under prefix "".
        let ws_member =
            Workspace::load(&root.join("apps/rust/Cookfile"), &root, &[]).unwrap();
        let reg_member = register_workspace(&ws_member, None, &[], RegisterMode::Enumerate, None).unwrap();

        let meta_of = |reg: &RegisteredWorkspace, key: &str| {
            reg.units_by_recipe
                .get(key)
                .unwrap_or_else(|| panic!("recipe '{key}' registered"))
                .units
                .first()
                .unwrap_or_else(|| panic!("recipe '{key}' has a unit"))
                .cache_meta
                .clone()
                .unwrap_or_else(|| panic!("recipe '{key}' unit has cache_meta"))
        };

        let meta_i = meta_of(&reg_root, "rust.build");
        let meta_ii = meta_of(&reg_member, "build");

        assert_eq!(
            meta_i.cookfile_path, meta_ii.cookfile_path,
            "cookfile_path must not depend on the entry point"
        );
        assert_eq!(
            meta_i.recipe_name, meta_ii.recipe_name,
            "recipe_name must not depend on the entry point"
        );
        assert_eq!(meta_i.cookfile_path, "apps/rust/Cookfile");
        assert_eq!(meta_i.recipe_name, "build");
    }

    /// Nested-import discovery: extras must be qualified with the LOCAL
    /// alias of the direct importer (a's `$<b.gen>`), and must NOT leak
    /// into members that don't import the discoverer directly (root).
    #[test]
    fn codegen_with_module_recipes_qualifies_extras_with_local_alias_only() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(
            dir.path().join("a/b/Cookfile"),
            "recipe b_static\n    cook \"b.o\" { echo $<out> }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a/Cookfile"),
            "import b ./b\nrecipe mid\n    cook \"mid.o\" { cat $<b.gen> > $<out> }\n",
        )
        .unwrap();
        // Root ALSO references `$<b.gen>` — but root does not import b
        // directly, so its reference must STAY mis-lowered (require_env)
        // after discovery: extras reach direct importers only.
        std::fs::write(
            dir.path().join("Cookfile"),
            "import a ./a\nrecipe top\n    cook \"top.o\" { cat $<a.mid> $<b.gen> > $<out> }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".cookroot"), "").unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
        let b_canon = std::fs::canonicalize(dir.path().join("a/b")).unwrap();
        ws.imports
            .get_mut(&b_canon)
            .unwrap()
            .lua_source
            .push_str("\ncook.recipe(\"gen\", {requires = {}}, function() end)\n");
        codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
        let a_canon = std::fs::canonicalize(dir.path().join("a")).unwrap();
        let a_lua = &ws.imports.get(&a_canon).unwrap().lua_source;
        assert!(
            a_lua.contains("cook.dep_output(\"b.gen\")"),
            "a's $<b.gen> must re-lower via its LOCAL alias, got:\n{a_lua}"
        );
        assert!(
            ws.root.lua_source.contains("cook.require_env(\"b.gen\")"),
            "root's $<b.gen> must stay mis-lowered (root does not import b \
             directly, so b's extras must not reach its union), got:\n{}",
            ws.root.lua_source
        );
        assert!(
            !ws.root.lua_source.contains("cook.dep_output(\"b.gen\")"),
            "root must not gain a dep_output for b.gen, got:\n{}",
            ws.root.lua_source
        );
    }
}
