//! The registration contract: surface names shared by emitter and
//! installer, and the summary data registration hands to its consumers.
//!
//! Every constant here is a wire format: cook-luagen EMITS it into generated
//! Lua, cook-register INSTALLS or READS it on the register VM. None of
//! them fails loudly on drift — a renamed global is simply never found
//! (`if let Ok` lookups silently skip; a `cook.`-table helper resolves to
//! nil and the generated call errors at runtime with no hint of why) — so
//! emitter and consumer MUST import the one constant (COOK-390, modeled
//! on REGISTER_SURFACE_NAME).
//!
//! The types are the registration summary (COOK-428): what the register
//! phase declared, as plain data — [`RegisteredWorkspace`] and the
//! [`RegisteredRecipePub`] family. cook-register produces them; cook-plan
//! aggregates them; cook-engine and cook-cli consume them. One definition
//! here stops the drift a per-crate mirror invites, exactly as it did for
//! [`MemberSourceDescriptor`] — and it lets the walk consume registration's
//! output without depending on the crate that runs registration: since this
//! move, cook-engine cannot name `register_cookfile` at all, which holds the
//! two-phase law (registration is closed before execute starts)
//! structurally rather than by convention.

/// Register-phase helper that records a surface `recipe NAME` block.
pub const REGISTER_SURFACE_NAME: &str = "__register_surface";

/// Register-phase helper that records a surface `chore NAME` block.
pub const REGISTER_SURFACE_CHORE_NAME: &str = "__register_surface_chore";

/// The generated program's registration entry point (CS-0172): codegen
/// wraps every registration in `function __cook_main() … end`; the engine
/// calls it after config dispatch. A silent-skip lookup — rename = the
/// entire registration body never runs.
pub const MAIN_PROGRAM_NAME: &str = "__cook_main";

/// The generated config-block dispatcher (CS-0163/CS-0172), called with the
/// selected config name before `__cook_main`. Same silent-skip lookup. The
/// name also appears in config-sandbox diagnostics.
pub const CONFIG_DISPATCH_NAME: &str = "__cook_run_config_blocks";

/// `cook.__probe_subst(ident)` — the CS-0195 register-time rendering of a
/// probe-value reference in an output pattern. Installed on the `cook`
/// table; emitted with the `cook.` receiver by luagen.
pub const PROBE_SUBST_NAME: &str = "__probe_subst";

/// `cook.__quote_param(value, name, ctx)` — the CS-0128 chore-parameter
/// quoting helper. Installed on the `cook` table; emitted with the `cook.`
/// receiver by luagen (which also encodes the 3-arg arity at its one
/// emission site).
pub const QUOTE_PARAM_NAME: &str = "__quote_param";

// ---------------------------------------------------------------------------
// __member_source — one shape, one set of key spellings (COOK-390)
// ---------------------------------------------------------------------------

/// The surface-meta field carrying a member-fanout recipe's data source
/// (`__member_source = { kind = "probe", ref = "…" }`, §22.5.10).
pub const MEMBER_SOURCE_FIELD: &str = "__member_source";

/// Key naming the descriptor's kind inside the meta table.
pub const MEMBER_SOURCE_KIND_KEY: &str = "kind";

/// Key naming the probe reference inside the meta table.
pub const MEMBER_SOURCE_REF_KEY: &str = "ref";

/// The only kind value since COOK-97 removed `Shell`/`Lua`.
pub const MEMBER_SOURCE_KIND_PROBE: &str = "probe";

/// The data source of a member-fanout recipe, as carried on the register
/// surface meta by `cook-luagen` and parsed back by `cook-register`'s
/// pre-pass (§22.5.10, COOK-64/COOK-190).
///
/// Until COOK-390 this shape was declared three times — the surface AST's
/// `cook_lang::MemberSource`, a register-crate mirror, and the emission
/// format string — with two hand-written conversions between them. The AST
/// type stays (it is the parser's), but emitter and consumer now share THIS
/// declaration and the key constants above.
///
/// - `Probe { source_ref }` — `ingredients <ref>`, the ref verbatim (`key`
///   or `key:field`; a probe key may itself be two-segment `ns:name`).
///   Resolution against the probe registry happens in the register
///   pre-pass (COOK-190); the body reads the resolved member array via
///   `cook.probes.get(<verbatim ref>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberSourceDescriptor {
    Probe { source_ref: String },
}

// ---------------------------------------------------------------------------
// The registration summary (COOK-428)
// ---------------------------------------------------------------------------

/// Whether a registered name is a normal recipe or a chore.
///
/// Chores are register-phase-only side-effecting blocks that
/// MUST NOT pass `cache = true` to `cook.add_unit` (§{chores.no-caching}).
/// Tracked on the public summary so consumers (CLI dispatch, surface
/// diagnostics) can branch on it without reaching into cook-register's
/// internal `RegisteredRecipe` shape.
///
/// This is the register-phase kind. It is deliberately distinct from
/// `cook_engine::RecipeKind`, the progress-event mirror enum: that one
/// answers "how should a completed recipe be labelled", this one answers
/// "what did the author declare".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeKind {
    Recipe,
    Chore,
}

/// How a recipe came to be registered.
///
/// - `Static` — emitted by codegen from a surface `recipe NAME` /
///   `chore NAME` block.
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
    /// It is a key NAME — a plain string, deliberately, so this type stays
    /// `Clone` and VM-free; the closure it names lives on the register VM.
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

/// Public summary of one registered recipe. Distinct from cook-register's
/// internal `capture::RegisteredRecipe` (which holds a `LuaRegistryKey`
/// closure that cannot cross the public API boundary).
#[derive(Debug, Clone)]
pub struct RegisteredRecipePub {
    pub name: String,
    pub source: RegistrationSource,
    pub kind: RecipeKind,
    pub requires: Vec<String>,
    /// Declared chore parameters (empty for normal recipes), carried through
    /// so `cook menu` can render `chore greet caller who="world"` instead of
    /// a bare name.
    pub params: Vec<ChoreParamMeta>,
    /// The module-qualified function name that minted this recipe (e.g.
    /// `"cook_pnpm.workspace"`), when the author opted in via the
    /// `cook.recipe(name, {origin = "..."}, body)` field. `None` when no
    /// `origin` was supplied, and always `None` for surface (`recipe NAME` /
    /// `chore NAME`) registrations — see cook-register's
    /// `capture::parse_origin_meta`.
    pub origin: Option<String>,
}

/// Workspace-level container that aggregates per-Cookfile registration
/// results into a single view: the value the plan (cook-plan) hands to the
/// walk (cook-engine). Names are qualified with their import prefix (root
/// Cookfile uses the empty prefix `""`).
///
/// Each Cookfile (root + each import) is registered independently by
/// `cook_register::register_cookfile`. cook-plan's `register_workspace`
/// merges names (with qualified prefix), units (keyed by qualified recipe
/// name), probes, and per-Cookfile working directories into this
/// workspace-wide view, which the engine's DAG builder and executor consume.
#[derive(Default)]
pub struct RegisteredWorkspace {
    pub warnings: Vec<String>,
    /// All recipes across all Cookfiles, names qualified with their import prefix.
    pub names: Vec<RegisteredRecipePub>,
    /// Per-recipe captured units, keyed by fully-qualified recipe name.
    pub units_by_recipe: std::collections::BTreeMap<String, crate::RecipeUnits>,
    /// Probes keyed by qualified probe key.
    pub probes: std::collections::BTreeMap<String, crate::ProbeUnit>,
    /// Per-Cookfile working directory, keyed by qualified prefix (`""` for root).
    pub working_dir_by_prefix: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Per-Cookfile `alias_dirs` (for `cook.dep_output` rewriting), keyed by
    /// qualified prefix (`""` for root) and then by alias name.
    pub alias_dirs_by_prefix:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, std::path::PathBuf>>,
    /// Snapshot of the register session's terminal-outputs map (recipe
    /// qualified-name → terminal output paths), taken after every Cookfile
    /// has registered. Threaded to the execute-phase worker VMs so
    /// `cook.dep_output` / `cook.dep_output_list` (§24.7, "Both") resolve
    /// there. The map is closed before execute phase starts, so a snapshot
    /// taken at the end of `register_workspace` is sound.
    pub terminal_outputs: std::collections::BTreeMap<String, Vec<String>>,
}
