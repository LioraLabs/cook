use mlua::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::cell::RefCell;

use cook_contracts::{
    CapturedUnit, DepKind, WorkPayload,
    REGISTER_SURFACE_CHORE_NAME, REGISTER_SURFACE_NAME,
};

use crate::{RecipeKind, SharedBodySlot};

/// Where a `RegisteredRecipe` came from. Used by Phase 2 collision detection
/// (and surface diagnostics) to name BOTH sites of a name conflict and
/// identify the kind of each.
///
/// - `Static`  — emitted by codegen from a surface `recipe NAME` /
///   `chore NAME` block via `cook.__register_surface(...)` or
///   `cook.__register_surface_chore(...)`. The `RecipeKind` carried alongside
///   on `RegisteredRecipe` distinguishes recipe from chore so
///   `detect_collisions` can label the site correctly.
/// - `Dynamic` — recorded by user / wrapper Lua code calling
///   `cook.recipe(...)` (e.g. `cook_cc.bin` target-makers). Always
///   recipe-kind: chores cannot be registered dynamically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationSource {
    /// Emitted by codegen from a surface `recipe NAME` block.
    Static { line: usize },
    /// Recorded by user / wrapper Lua code calling `cook.recipe(...)`.
    Dynamic { line: usize },
}

pub struct RegisteredRecipe {
    pub name: String,
    pub function: LuaRegistryKey,
    pub metadata: RegisteredMetadata,
    pub source: RegistrationSource,
    /// Whether the registered name is a normal recipe or a chore.
    ///
    /// Codegen sets `Chore` only via `cook.__register_surface_chore`
    /// (surface `chore NAME` blocks). All other registration paths
    /// (`cook.recipe`, `cook.__register_surface`) set `Recipe`.
    pub kind: RecipeKind,
    /// COOK-64 §22.5.10: the member source of a surface member-fanout
    /// recipe, captured from the `__member_source` field codegen emits on the
    /// register surface meta. `None` for recipes with no member source
    /// and for every non-surface registration path (`cook.recipe`, chores).
    /// The register pre-pass reads this to evaluate a feeding probe ahead of
    /// the body invocation that materialises the member set.
    pub member_source: Option<MemberSourceDescriptor>,
}

/// The data source of a member-fanout recipe, as carried on the register
/// surface meta (`__member_source = {kind=…}`) by `cook-luagen`. Mirrors the
/// surface-AST `MemberSource` but lives in the register crate so the
/// pre-pass can dispatch without a parser dependency.
///
/// - `Probe { source_ref }` — `ingredients <ref>`, the ref verbatim (`key`
///   or `key:field`; a probe key may itself be two-segment `ns:name`).
///   Resolution against the probe registry happens in the register
///   pre-pass (COOK-190); the body reads the resolved member array via
///   `cook.probes.get(<verbatim ref>)`.
///
/// The `Shell { cmd, as_lines }` and `Lua` variants have been removed in
/// COOK-97 — only `Probe` remains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberSourceDescriptor {
    Probe { source_ref: String },
}

/// Parse the `__member_source` descriptor off a register surface meta table.
/// Returns `None` when the field is absent (a non-member-fanout recipe).
fn parse_member_source_meta(meta: &LuaTable) -> LuaResult<Option<MemberSourceDescriptor>> {
    let Some(t) = meta.get::<Option<LuaTable>>("__member_source")? else {
        return Ok(None);
    };
    let kind: String = t.get("kind")?;
    Ok(Some(match kind.as_str() {
        "probe" => MemberSourceDescriptor::Probe { source_ref: t.get("ref")? },
        other => {
            return Err(mlua::Error::runtime(format!(
                "cook.__register_surface: unknown __member_source kind '{other}'"
            )))
        }
    }))
}

/// One parameter declared in a `chore NAME param …` header.
///
/// Mirrors the `kind` strings emitted by `cook-luagen` into the
/// `__params` metadata table.
#[derive(Debug, Clone)]
pub enum ChoreParamMeta {
    /// A required positional — must be supplied by argv.
    Required { name: String },
    /// A defaulted positional — falls back to `default` when argv
    /// is exhausted at this position.
    DefaultedString { name: String, default: String },
    /// A defaulted positional with a Lua-expression default — evaluates
    /// the closure when argv is exhausted at this position.
    ///
    /// `default_key_name` is a named-registry key (set via
    /// `lua.set_named_registry_value`) referencing the closure
    /// `function() return (EXPR) end` emitted by codegen. Retrieved at
    /// binding time via `lua.named_registry_value::<LuaFunction>(&name)`.
    ///
    /// Named registry keys use a unique string per registration pass;
    /// the key is `"__cook_chore_default:<chore>:<param>:<serial>"`.
    DefaultedLua { name: String, default_key_name: String },
    /// A one-or-more variadic — collects all remaining argv into a Lua sequence;
    /// zero remaining argv is an error.
    VariadicPlus { name: String },
    /// A zero-or-more variadic — collects all remaining argv into a Lua sequence;
    /// zero remaining argv binds to an empty table.
    VariadicStar { name: String },
}

impl ChoreParamMeta {
    /// The parameter name (for binding into the Lua table).
    pub fn param_name(&self) -> &str {
        match self {
            ChoreParamMeta::Required { name } => name,
            ChoreParamMeta::DefaultedString { name, .. } => name,
            ChoreParamMeta::DefaultedLua { name, .. } => name,
            ChoreParamMeta::VariadicPlus { name } => name,
            ChoreParamMeta::VariadicStar { name } => name,
        }
    }

    /// Human-readable token for `cook menu` display, e.g. `caller`,
    /// `who="world"`, `tail...`, `[rest...]`. Mirrors the `chore NAME …`
    /// header syntax closely enough to be recognisable, without claiming
    /// to be a re-parseable grammar.
    pub fn display_token(&self) -> String {
        match self {
            ChoreParamMeta::Required { name } => name.clone(),
            ChoreParamMeta::DefaultedString { name, default } => format!("{name}={default:?}"),
            ChoreParamMeta::DefaultedLua { name, .. } => format!("{name}=<lua>"),
            ChoreParamMeta::VariadicPlus { name } => format!("{name}..."),
            ChoreParamMeta::VariadicStar { name } => format!("[{name}...]"),
        }
    }
}

#[derive(Debug)]
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub requires: Vec<String>,
    /// Ordered list of declared chore parameters. Empty for normal
    /// recipes (which do not take parameters).
    pub params: Vec<ChoreParamMeta>,
    /// The module-qualified function name that minted this recipe (e.g.
    /// `"cook_pnpm.workspace"`), when the author opted in via the
    /// `cook.recipe(name, {origin = "..."}, body)` field. `None` when no
    /// `origin` was supplied, and ALWAYS `None` for surface registrations
    /// (`cook.__register_surface` / `cook.__register_surface_chore`) — see
    /// `parse_origin_meta`.
    pub origin: Option<String>,
}

/// Parse the (ingredients, excludes, requires) string-list fields from a
/// Lua metadata table. Missing or non-table values yield empty vectors;
/// individual non-string entries are silently skipped (matches the historical
/// inline parser in `cook.recipe`).
///
/// Shared by `cook.recipe`, `cook.__register_surface`, and
/// `cook.__register_surface_chore` so the three registration paths see
/// identical metadata semantics.
fn parse_meta_lists(meta: &LuaTable) -> LuaResult<(Vec<String>, Vec<String>, Vec<String>)> {
    let mut ingredients = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("ingredients") {
        for pair in t.sequence_values::<String>() {
            if let Ok(s) = pair {
                ingredients.push(s);
            }
        }
    }
    let mut excludes = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("excludes") {
        for pair in t.sequence_values::<String>() {
            if let Ok(s) = pair {
                excludes.push(s);
            }
        }
    }
    let mut requires = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("requires") {
        for pair in t.sequence_values::<String>() {
            if let Ok(s) = pair {
                requires.push(s);
            }
        }
    }
    Ok((ingredients, excludes, requires))
}

/// Uniform register-phase type error for a `cook.recipe` field: a
/// wrong-typed field is a hard error naming the field, the expected type,
/// and the received Lua type — never silently coerced to its default.
///
/// Applies the field-typing discipline `cook.add_unit` established under
/// CS-0127 to `cook.recipe`'s metadata table. The citation is CS-0143, not
/// CS-0127: CS-0127 covers §22.1/§22.4/§22.5.7 and says nothing about
/// `cook.recipe` or `origin`, so pointing a reader at it would send them to
/// an entry that does not describe the error they hit. CS-0143 is the entry
/// that specifies §22.3's `origin` key and this rejection.
///
/// Mirrors `unit_api.rs::type_err`, which hardcodes the `cook.add_unit:`
/// prefix — hence a sibling rather than a reuse.
///
/// `api` names the calling surface (`cook.recipe` or `cook.chore`, CS-0176).
/// It is a parameter rather than a hardcoded prefix because `cook.chore`
/// shares `parse_origin_meta`: reporting `cook.recipe:` for a bad `origin`
/// passed to `cook.chore` would name a function the author never called.
fn recipe_type_err(api: &str, field: &str, expected: &str, got: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "{api}: `{field}` must be {expected}, got {got} (Standard \u{00a7}22.3, CS-0143)"
    ))
}

/// Parse the `origin` field off a `cook.recipe` metadata table.
///
/// `origin` is an optional, explicit opt-in annotation (convention: the
/// module-qualified function name that minted the recipe, e.g.
/// `"cook_pnpm.workspace"`) that lets `cook list` attribute a
/// module-minted recipe back to the call that registered it.
///
/// - Absent, or explicitly `nil` → `Ok(None)`.
/// - A non-empty Lua string → `Ok(Some(s))`.
/// - An empty string → a register error. An empty origin would render as
///   `(from )`, which is worse than no annotation at all, so it is treated
///   as a wrong-typed-adjacent authoring mistake rather than accepted.
/// - Any other type (number, boolean, table, function) → a register error
///   naming the field and the offending Lua type.
///
/// Deliberately NOT folded into `parse_meta_lists`: that helper is shared
/// with `cook.__register_surface` / `cook.__register_surface_chore`, and a
/// surface `recipe NAME` / `chore NAME` block must never acquire an origin
/// — only the public `cook.recipe` / `cook.chore` closures call this
/// function, so that guarantee is structural rather than a matter of not
/// passing the field.
fn parse_origin_meta(api: &str, meta: &LuaTable) -> LuaResult<Option<String>> {
    match meta.get::<LuaValue>("origin")? {
        LuaValue::Nil => Ok(None),
        LuaValue::String(s) => {
            let s = s.to_string_lossy().to_string();
            if s.is_empty() {
                return Err(recipe_type_err(
                    api,
                    "origin",
                    "a non-empty string",
                    "an empty string",
                ));
            }
            Ok(Some(s))
        }
        other => Err(recipe_type_err(api, "origin", "a string", other.type_name())),
    }
}

/// Validate a `cook.chore` name against the identity of the module that is
/// registering it (Standard §12.7.8 chore carve-out, CS-0176).
///
/// §12.7.8's name-ownership MUST — "a recipe's name, and the fact that it is
/// invocable, belong to the author of the Cookfile" — is scoped to *recipes*.
/// The carve-out for chores rests entirely on the namespace prefix: a recipe
/// is a build target in the DAG the author owns, whereas `cc.add` is a tool
/// the module offers under a name the author can attribute at a glance. The
/// dotted prefix is what makes that hold rather than merely assert it, so it
/// is REQUIRED and checked, not conventional.
///
/// `module` is the module currently being evaluated. It is deliberately
/// `current_module` and never `ModuleLoaderState::active_module()`: the
/// latter falls back to `last_module` ("most recently loaded"), which is not
/// "the module that owns the running function". With `use cook_cc` followed
/// by `use cook_pnpm`, a call made from a cook_cc function after both loads
/// reports `cook_pnpm` — checking a prefix against that would hollow the
/// carve-out into an assertion again. Requiring registration *during* module
/// evaluation makes the identity exact by construction.
///
/// Accepted prefixes are the module name itself and the module name with a
/// leading `cook_` stripped: module `cook_cc` admits both `cook_cc.add` and
/// `cc.add`. The strip exists because blessed modules are named `cook_*` by
/// convention while their verbs read as `cc.*` / `pnpm.*`.
fn validate_chore_namespace(name: &str, module: Option<&str>) -> LuaResult<()> {
    let module = module.ok_or_else(|| {
        mlua::Error::runtime(format!(
            "cook.chore: '{name}' was registered outside module evaluation. A chore's \
             namespace is checked against the module registering it, and that identity \
             is only exact while the module's own chunk is running — register chores at \
             module top level, not from a function called later by a Cookfile \
             (Standard \u{00a7}12.7.8, CS-0176)"
        ))
    })?;

    let short = module.strip_prefix("cook_").unwrap_or(module);
    let prefix = name.split('.').next().unwrap_or("");

    if prefix.is_empty() || !name.contains('.') {
        return Err(mlua::Error::runtime(format!(
            "cook.chore: '{name}' must be namespaced. A module-registered chore takes a \
             dotted name whose first segment is its module: try '{short}.{name}' \
             (Standard \u{00a7}12.7.8, CS-0176)"
        )));
    }
    if prefix != module && prefix != short {
        return Err(mlua::Error::runtime(format!(
            "cook.chore: '{name}' claims namespace '{prefix}', but module '{module}' may \
             only register under '{short}' or '{module}' \
             (Standard \u{00a7}12.7.8, CS-0176)"
        )));
    }
    Ok(())
}

/// Next serial for named-registry keys used by `DefaultedLua` params.
///
/// Each `defaulted_lua` parameter stores its default closure under a unique
/// named-registry key so that `ChoreParamMeta` remains `Clone` (unlike
/// `mlua::RegistryKey` which is not `Clone`). The key is scoped to the
/// current Lua VM instance; a fresh counter value is assigned for each
/// parameter at registration time.
///
/// COOK-36 known stub: `COUNTER` is process-global (`static AtomicU64`) and
/// never decrements. Each `register_cookfile` pass currently builds a fresh
/// Lua VM, so named-registry slots from older VMs are GC'd correctly when
/// the VM is dropped — there is no leak under normal CLI runs. If future
/// work caches the Lua VM across multiple register passes (e.g. a long-
/// running watch loop reusing one VM), each pass for a chore with Lua-
/// expression defaults will allocate new registry slots without reclaiming
/// the old ones. A small cleanup helper that purges keys matching the
/// `__cook_chore_default:` prefix after each pass would close that gap.
fn next_lua_default_serial() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Extract the `__params` metadata array from a chore's registration table.
///
/// Returns an empty `Vec` when `meta` has no `__params` key (i.e. a recipe or
/// a chore with no declared parameters). Iterates the sequence and dispatches
/// on the `kind` string field:
///
/// - `"required"` → `ChoreParamMeta::Required { name }`
/// - `"defaulted_string"` → `ChoreParamMeta::DefaultedString { name, default }`
/// - `"defaulted_lua"` → `ChoreParamMeta::DefaultedLua { name, default_key_name }`
/// - `"variadic_plus"` → `ChoreParamMeta::VariadicPlus { name }`
/// - `"variadic_star"` → `ChoreParamMeta::VariadicStar { name }`
/// - anything else → runtime error
///
/// For `defaulted_lua`, the `default` field (a Lua function) is stored in the
/// named registry under a unique key, and the key name is recorded on the
/// `ChoreParamMeta` so that `build_chore_params_table` can retrieve it.
fn parse_chore_params_meta(lua: &Lua, meta: &LuaTable) -> LuaResult<Vec<ChoreParamMeta>> {
    let params_tbl = match meta.get::<Option<LuaTable>>("__params")? {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for pair in params_tbl.sequence_values::<LuaTable>() {
        let entry = pair?;
        let kind: String = entry.get("kind")?;
        let name: String = entry.get("name")?;
        match kind.as_str() {
            "required" => {
                out.push(ChoreParamMeta::Required { name });
            }
            "defaulted_string" => {
                let default: String = entry.get("default")?;
                out.push(ChoreParamMeta::DefaultedString { name, default });
            }
            "defaulted_lua" => {
                let func: LuaFunction = entry.get("default")?;
                let serial = next_lua_default_serial();
                let key_name = format!("__cook_chore_default:{}:{}", name, serial);
                lua.set_named_registry_value(&key_name, func)?;
                out.push(ChoreParamMeta::DefaultedLua { name, default_key_name: key_name });
            }
            "variadic_plus" => {
                out.push(ChoreParamMeta::VariadicPlus { name });
            }
            "variadic_star" => {
                out.push(ChoreParamMeta::VariadicStar { name });
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "chore parameter kind '{other}' is not supported"
                )));
            }
        }
    }
    Ok(out)
}

/// Install the register-phase `cook.*` API surface on the given Lua VM.
/// This is the whole namespace (recipe registration, capture-mode
/// `cook.exec`/`cook.sh`, etc.), not just `cook.recipe`.
pub fn install_cook_api(
    lua: &Lua,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    working_dir: &PathBuf,
    body_slot: SharedBodySlot,
    recipe_name: &str,
    module_state: crate::module_loader::SharedModuleLoaderState,
) -> LuaResult<Rc<RefCell<Vec<RegisteredRecipe>>>> {
    let recipes: Rc<RefCell<Vec<RegisteredRecipe>>> = Rc::new(RefCell::new(Vec::new()));
    let cook = lua.create_table()?;

    // cook.recipe(name, metadata, fn) — the public API.
    // Always tagged Dynamic; chores cannot be registered through this path.
    let recipes_clone = recipes.clone();
    let recipe_fn =
        lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            let origin = parse_origin_meta("cook.recipe", &meta)?;
            let line = caller_line_in_cookfile(lua).unwrap_or(0);

            recipes_clone.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params: vec![],
                    origin,
                },
                source: RegistrationSource::Dynamic { line },
                kind: RecipeKind::Recipe,
                // Dynamic `cook.recipe` registrations carry no surface
                // member source — that lowering is surface-only.
                member_source: None,
            });
            Ok(())
        })?;
    cook.set("recipe", recipe_fn)?;

    // cook.chore(name, meta, fn) — the public module-facing API (CS-0176).
    //
    // Sibling to `cook.recipe`, tagged `RecipeKind::Chore` and `Dynamic`.
    // Exists so a blessed module can offer verbs (`cc.add`, `cc.link`) that
    // a Cookfile author invokes without having declared them — the thing a
    // surface `chore NAME` block cannot express, since the author would have
    // to write the block themselves.
    //
    // The name MUST be namespaced to the registering module; see
    // `validate_chore_namespace` for why that is load-time-only and why the
    // identity comes from `current_module` rather than `active_module()`.
    //
    // Note the asymmetry with `cook.recipe` above: chore *unit* semantics
    // (no-cache, interactive) are NOT established here. For a surface chore
    // they come from codegen wrapping the body in `cook._enter_chore()` /
    // `cook._exit_chore()`; a Lua-registered body has no such wrapper, so the
    // engine brackets the call itself at the `RecipeKind::Chore` branch of
    // `invoke_body`. Registering here without that bracket would silently
    // yield cacheable, non-interactive units — a §7.4 violation that no test
    // of this closure alone would catch.
    let recipes_dyn_chore = recipes.clone();
    let chore_module_state = module_state.clone();
    let chore_pub_fn =
        lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            {
                let state = chore_module_state.borrow();
                validate_chore_namespace(&name, state.current_module.as_deref())?;
            }
            let key = lua.create_registry_value(func)?;
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            let params = parse_chore_params_meta(lua, &meta)?;
            let origin = parse_origin_meta("cook.chore", &meta)?;
            let line = caller_line_in_cookfile(lua).unwrap_or(0);

            recipes_dyn_chore.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params,
                    origin,
                },
                source: RegistrationSource::Dynamic { line },
                kind: RecipeKind::Chore,
                // Chores cannot declare a member source (§8.2 is recipe-only).
                member_source: None,
            });
            Ok(())
        })?;
    cook.set("chore", chore_pub_fn)?;

    // cook.__register_surface(name, meta, body) — codegen-private API.
    //
    // Emitted by `cook-luagen` for surface `recipe NAME` blocks. Distinct
    // from `cook.recipe` (which tags Dynamic) so collision diagnostics can
    // identify a surface declaration vs. a register-phase Lua call by source
    // kind, not just by line. The `__line = N` field in `meta` carries the
    // Cookfile source line of the surface block; the Lua call-stack walk
    // used by `cook.recipe` is not the right answer here because the codegen
    // splices into the top-level chunk and the call site line is the
    // generated chunk line, not the Cookfile source line.
    //
    // Not part of the public Cook Lua API (CS-0077 §6.4 implementation note).
    let recipes_surface = recipes.clone();
    let surface_fn = lua.create_function(
        move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;
            // `__line` is always written by codegen (`generate_metadata_with_line`).
            // The `unwrap_or(0)` is defensive — a hand-typed
            // `cook.__register_surface` call without the field would land 0,
            // matching the legacy `cook.recipe` "no line info" sentinel.
            let line: usize = meta.get("__line").unwrap_or(0);
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            let member_source = parse_member_source_meta(&meta)?;
            recipes_surface.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params: vec![],
                    // Surface `recipe NAME` blocks never carry an origin —
                    // only the public `cook.recipe` closure parses it.
                    origin: None,
                },
                source: RegistrationSource::Static { line },
                kind: RecipeKind::Recipe,
                member_source,
            });
            Ok(())
        },
    )?;
    cook.set(REGISTER_SURFACE_NAME, surface_fn)?;

    // cook.__register_surface_chore(name, meta, body) — codegen-private API.
    //
    // Same shape as `cook.__register_surface` but tagged `RecipeKind::Chore`.
    // Emitted by `cook-luagen` for surface `chore NAME` blocks. Chores have
    // no `ingredients`/`excludes` (parser guarantees), but the helper parses
    // them defensively to keep one code path for metadata extraction.
    let recipes_chore = recipes.clone();
    let chore_fn = lua.create_function(
        move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;
            let line: usize = meta.get("__line").unwrap_or(0);
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            let params = parse_chore_params_meta(lua, &meta)?;
            recipes_chore.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params,
                    // Surface `chore NAME` blocks never carry an origin —
                    // only the public `cook.recipe` closure parses it.
                    origin: None,
                },
                source: RegistrationSource::Static { line },
                kind: RecipeKind::Chore,
                // Chores cannot declare a member source (§8.2 is recipe-only).
                member_source: None,
            });
            Ok(())
        },
    )?;
    cook.set(REGISTER_SURFACE_CHORE_NAME, chore_fn)?;

    // cook.exec(cmd, line) — capture mode
    let body_slot_exec = body_slot.clone();
    let exec_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut slot = body_slot_exec.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.exec called outside a recipe body")
        })?;
        if body.inside_layer {
            body.layer_commands.push((cmd, line));
        } else {
            let unit = CapturedUnit {
                payload: WorkPayload::Shell {
                    cmd: cmd.clone(),
                    line,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                test_name: None,
                output_paths: Vec::new(),
            };
            body.units.push(unit);
        }
        Ok("".to_string())
    })?;
    cook.set("exec", exec_fn)?;

    // cook.interactive(cmd, line) — capture mode
    let body_slot_i = body_slot.clone();
    let interactive_capture_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut slot = body_slot_i.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.interactive called outside a recipe body")
        })?;
        let unit = CapturedUnit {
            payload: WorkPayload::Interactive {
                cmd: cmd.clone(),
                line,
                is_chore: false,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            test_name: None,
            output_paths: Vec::new(),
        };
        body.units.push(unit);
        Ok("".to_string())
    })?;
    cook.set("interactive", interactive_capture_fn)?;

    // cook.sh(cmd) — capture mode: inside a layer it captures like exec;
    // outside a layer it actually executes (user-facing utility that returns stdout).
    //
    // cook.sh has a long-standing top-level use as a utility (e.g. version
    // detection in module init code that returns the stdout). When called
    // without an active body slot, behave as the "execute immediately" path:
    // there is no layer context outside a body anyway, so this preserves the
    // existing surface. Inside a body, the layer check applies as before.
    let body_slot_sh = body_slot.clone();
    let wd_sh = working_dir.clone();
    let sh_recipe_name = recipe_name.to_string();
    let sh_fn = lua.create_function(move |_, cmd: String| {
        {
            let mut slot = body_slot_sh.borrow_mut();
            if let Some(body) = slot.as_mut() {
                if body.inside_layer {
                    body.layer_commands.push((cmd, 0));
                    return Ok("".to_string());
                }
            }
        }
        // Execute immediately — cook.sh is a user-facing utility
        // and callers depend on its return value for control flow.
        //
        // CS-0172: the child inherits the ambient environment and nothing
        // else. A declared variable is not a process-environment entry
        // (§5.3.1) — injecting one here would make `$NAME` in a `cook.sh`
        // command silently resolve to a build variable, which is the
        // conflation this CS removes. `$<NAME>` interpolates one explicitly.
        run_shell_command(&cmd, &wd_sh, &HashMap::new(), 0, &sh_recipe_name)
    })?;
    cook.set("sh", sh_fn)?;

    // CS-0172: the declared-variable store. Config blocks write it (as their
    // `var` sink, §5.3.1); everything else reads it through the read-only
    // `var` global installed below. The store itself is kept in the Lua
    // registry rather than on `cook`, so the only Lua-reachable handle is the
    // one with the read-only guard on it — a recipe body cannot reach past the
    // proxy and mutate a declared value out from under the cache key.
    let var_store = lua.create_table()?;
    {
        let snap = env_vars.borrow();
        for (key, value) in snap.iter() {
            var_store.set(key.as_str(), value.as_str())?;
        }
    }
    // The read-only `var` global that reads this store is installed alongside
    // `cook.require_var`, where the declared keyset is in scope.
    lua.set_named_registry_value(crate::VAR_STORE_REGISTRY_KEY, var_store)?;

    // COOK-64 §9.3: cook.member_to_string(value) renders a member fan-out
    // member to its canonical string form (key-sorted JSON for a table, the
    // scalar's bare string otherwise). Bound on the register VM so the fan-out
    // codegen's `member = cook.member_to_string(item)` and `$<in>` resolve.
    let member_fn = lua.create_function(|_, value: mlua::Value| {
        let jv = crate::probe_value::lua_to_json(&value)
            .map_err(|e| mlua::Error::runtime(format!("cook.member_to_string: {e}")))?;
        Ok(cook_contracts::member::member_to_string(&jv))
    })?;
    cook.set("member_to_string", member_fn)?;

    // cook.__quote_param(value, name, ctx) — runtime helper for chore parameter
    // placeholders. Luagen's normal sigil resolver decides which `$<NAME>`
    // tokens are params; this helper only quotes the already-bound value.
    // `ctx` (CS-0128) is the shell quoting context Luagen scanned for the sigil
    // span ("bare" | "dquote" | "squote"); it defaults to "bare".
    let quote_param_fn =
        lua.create_function(move |_, (value, name, ctx): (mlua::Value, String, Option<String>)| -> mlua::Result<String> {
            let ctx = ctx.as_deref().unwrap_or("bare");
            match value {
                mlua::Value::String(s) => Ok(quote_for_ctx(&s.to_str()?, ctx)),
                mlua::Value::Table(t) => {
                    let mut parts: Vec<String> = Vec::new();
                    for value in t.sequence_values::<mlua::Value>() {
                        match value? {
                            mlua::Value::String(s) => parts.push(quote_for_ctx(&s.to_str()?, ctx)),
                            other => {
                                return Err(mlua::Error::runtime(format!(
                                    "chore parameter '$<{name}>' contains non-string element of type {}",
                                    other.type_name()
                                )));
                            }
                        }
                    }
                    Ok(parts.join(" "))
                }
                mlua::Value::Nil => Err(mlua::Error::runtime(format!(
                    "chore parameter '$<{name}>' is not bound (BUG: codegen emitted a param ref for an undeclared name)"
                ))),
                other => Err(mlua::Error::runtime(format!(
                    "chore parameter '$<{name}>' has unexpected type {}",
                    other.type_name()
                ))),
            }
        })?;
    cook.set("__quote_param", quote_param_fn)?;

    lua.globals().set("cook", cook)?;
    Ok(recipes)
}

/// CS-0128: quote `s` for the shell context `ctx` that a `$<param>` sigil
/// occupies in the step text.
///
/// * `"bare"` (default) — single-quote the whole value for word-safety
///   (the existing `shell_quote` behaviour).
/// * `"dquote"` — the sigil sits inside an author-supplied double-quoted
///   region, so emit the value with double-quote-context escaping
///   (backslash-escape `\`, `"`, `$`, and backtick); the surrounding `"..."`
///   already provides the quoting.
/// * `"squote"` — the sigil sits inside an author-supplied single-quoted
///   region, so emit the raw value verbatim; a value containing `'` is the
///   author's responsibility (documented edge).
fn quote_for_ctx(s: &str, ctx: &str) -> String {
    match ctx {
        "dquote" => {
            let mut o = String::with_capacity(s.len());
            for ch in s.chars() {
                if matches!(ch, '\\' | '"' | '$' | '`') {
                    o.push('\\');
                }
                o.push(ch);
            }
            o
        }
        "squote" => s.to_string(),
        _ => shell_quote(s),
    }
}

/// POSIX-safe single-quote escaping for shell arguments.
///
/// Wraps the whole string in single quotes; any literal `'` becomes `'\''`
/// (close-quote, escaped-quote, re-open-quote). This is the canonical
/// sh-portable form and handles every character including spaces, backslashes,
/// and dollar signs.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Upper bound on the Lua call-stack walk in `caller_line_in_cookfile`.
/// A safety cap — 40 frames comfortably exceeds any realistic Cookfile
/// call chain; the early `None` return on missing frames is the
/// expected termination.
const MAX_LUA_STACK_DEPTH: usize = 40;

/// Walk the Lua call stack and return the line number of the topmost frame
/// whose source string matches the Cookfile path label set by
/// `__cook_cookfile_path` (or any module loaded via `module_loader` with a
/// `@{module_path}` chunk name that ends with the cookfile-relative label).
/// Returns `None` if the Cookfile frame can't be located.
///
/// Used by `cook.recipe` to tag each `RegisteredRecipe` with the line number
/// of the user-code site that registered it. When the registry value isn't
/// populated (legacy/test call sites) or the matching frame can't be found,
/// callers default to `line = 0`.
fn caller_line_in_cookfile(lua: &Lua) -> Option<usize> {
    let target: String = lua
        .named_registry_value::<String>("__cook_cookfile_path")
        .ok()?;

    // Lua call levels: 1 = the closure, 2 = the caller, 3+ = caller's caller, ...
    for level in 1..MAX_LUA_STACK_DEPTH {
        match lua.inspect_stack(level) {
            None => return None,
            Some(dbg) => {
                let src_opt = dbg.source().source;
                let source: &str = src_opt.as_deref().unwrap_or("");
                // Module-loaded chunks have an "@" prefix (see module_loader.rs); the
                // `__cook_cookfile_path` registry value does not. Match either form.
                if source == target || source.ends_with(&target) {
                    return Some(dbg.curr_line() as usize);
                }
            }
        }
    }
    None
}

/// `cook.sh` at register phase (§{lua.cook-sh}).
///
/// The twin of `cook_luaotp::pool`'s worker-phase implementation, and this
/// milestone opened by naming them: "Command-failure formatting was fixed in
/// one producer while its twin remained broken." They are no longer twins.
/// Both call the one primitive, which builds the `CommandFailure` for both, so
/// a change to how a failure reads can no longer reach one and miss the other.
///
/// The register phase deliberately does NOT disarm `cook-fingerprint`'s stat
/// memo. Registration is capture mode (§{exec.capture}): a register-phase
/// `cook.sh` must not modify files under the working directory in a way that
/// affects declared outputs, so there is nothing for the memo to have gone
/// stale against. That asymmetry is exactly why disarming belongs to the
/// callers rather than to `cook-shell`.
fn run_shell_command(
    cmd: &str,
    wd: &std::path::Path,
    env: &HashMap<String, String>,
    line: usize,
    _recipe_name: &str,
) -> mlua::Result<String> {
    let outcome = cook_shell::run(
        &cook_shell::Spawn { command: cmd, working_dir: wd, stdio: cook_shell::Stdio::Captured },
        env,
    )
    .map_err(|e| mlua::Error::runtime(e.message().to_string()))?;

    if let Some(failure) = outcome.failure(line, cmd) {
        return Err(mlua::Error::runtime(failure.to_wire()));
    }
    Ok(outcome.stdout_lossy())
}

#[cfg(test)]
#[path = "tests/display_token_tests.rs"]
mod display_token_tests;

#[cfg(test)]
#[path = "tests/capture_tests.rs"]
mod capture_tests;
