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
//!
//! # The residual, named here rather than left to expire
//!
//! `cook-luagen` reads these constants wherever the emission is a call it
//! COMPOSES ([`door_call`]). It does NOT read them inside its multi-line
//! `format!` templates — `cook.add_unit{…}`, `cook.step_group(function() …`,
//! `cook.member_to_string(item)`, `cook.prior_outputs(…)`,
//! `cook.passthrough(…)` — where the door's argument is a Lua EXPRESSION
//! rather than a string, so `door_call` would quote it, and where
//! interpolating a constant into a five-line literal costs more legibility
//! than it buys. The constitution's duplicate-literal rule cannot see a name
//! buried in a literal that size either, so nothing mechanical was going to
//! notice.
//!
//! That is a deliberate copy, and the deliberate-copy protocol requires an
//! agreement test in place of the missing edge:
//! `cook-luagen/src/tests/door_names_tests.rs` collects every `cook.<door>`
//! spelling in that crate and fails unless each is a constant declared here or
//! an exception listed there with a reason. A door renamed here while a
//! template keeps the old spelling fails that test instead of failing a user's
//! build at run time, as a call to a nil value.

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

/// Implementation-private registration door for compiler-generated inline
/// file determinants. The public `cook.probe` door rejects this key namespace.
pub const INLINE_SEAL_PROBE_NAME: &str = "__inline_seal_probe";

/// A call to a door on the `cook` table with one string argument, qualified
/// and escaped: `door_call(PROBE_SUBST_NAME, ident)` is
/// `cook.__probe_subst("…")`.
///
/// A door name is shared because drift in it is silent (see this module's
/// header, and [`crate::module_binding::LOAD_MODULE_FN`], which is a door of
/// the same kind kept next to the `use` rules that name it). The CALL around a
/// name was not shared: `cook-luagen` composed `format!("cook.{}(\"{}\")", …)`
/// at three sites and `module_binding` at a fourth, each pairing a shared
/// constant with a privately spelled receiver, argument list and escape. The
/// gate that flagged it saw only the repeated format string (COOK-437);
/// COOK-440 is the finding that it was one decision written four times.
///
/// Two crates compose door calls — this one and `cook-luagen` — which is why
/// the composer is here rather than in the emitter. Be clear about what that
/// buys and what it does not: it makes the four *emissions* agree with each
/// other, and it puts the escape on the path so a `"` or a carriage return in
/// an ident cannot end the literal and fail the whole program's load. It does
/// NOT yet make the emitter agree with the installer, which sets the field
/// from its own `set(NAME, …)` on the register VM and is bound to this only by
/// the constant. Closing that half is COOK-439's, and it will find the emitting
/// half already in one place.
///
/// Doors taking more than one argument keep their own `format!`: the arity is
/// part of what each door means, `__quote_param`'s third argument is a quoting
/// context rather than a Lua string, and a composer generalised over arity
/// before a second caller needs it would be an abstraction invented for a need
/// nothing has.
pub fn door_call(name: &str, arg: &str) -> String {
    format!("cook.{}({})", name, crate::lua_string::literal(arg))
}

/// `cook.__probe_subst(ident)` for the CS-0195 register-time rendering of a
/// probe-value reference, composed once.
///
/// `cook-luagen` emits this from three places — a command body, an output
/// pattern, and a fan-out test command — each carrying a comment claiming one
/// renderer per ident. This is that renderer.
pub fn probe_subst_call(ident: &str) -> String {
    door_call(PROBE_SUBST_NAME, ident)
}

/// `cook.__quote_param(value, name, ctx)` — the CS-0128 chore-parameter
/// quoting helper. Installed on the `cook` table; emitted with the `cook.`
/// receiver by luagen (which also encodes the 3-arg arity at its one
/// emission site).
pub const QUOTE_PARAM_NAME: &str = "__quote_param";

// ---------------------------------------------------------------------------
// The two VMs' door names (COOK-439 / CS-0213)
// ---------------------------------------------------------------------------
//
// Cook runs two Lua VMs, one per phase: `cook-register` hosts the register
// VM, `cook-execute` hosts the execute-phase worker VMs. The Standard says
// which doors exist in which phase (§6.3.2, §13.2, §24.7), and BOTH VMs have
// to spell every name it mentions — the one that implements the door, and the
// one that installs the §6.3.2 guard refusing it. Until COOK-439 each spelled
// its own.
//
// Drift here does not fail to compile and does not fail to link. A renamed
// door is simply a different door: the guard stops guarding, the call resolves
// to nil, and what a Cookfile may call in one phase silently stops matching
// what it may call in the other. That is the same silent-drift argument this
// module's header makes for the emitter/installer constants above, applied to
// the second pair of ends.

/// `cook.add_unit(tbl)` — the register-phase declaration of one work unit
/// (§{lua.add-unit}). `cook-register` installs the recorder; `cook-execute`
/// installs the §6.3.2 register-only guard under the same name.
pub const ADD_UNIT_NAME: &str = "add_unit";

/// `cook.step_group(fn, opts?)` — the register-phase step-group opener
/// (§{lua.step-group}). Installed as the recorder by `cook-register`, as the
/// §6.3.2 guard by `cook-execute`, and rendered by `cook-graph` as the wire
/// label of [`crate::DepKind::StepGroup`] — see [`crate::DepKind::wire_name`],
/// which is what keeps the third end from being a third spelling.
pub const STEP_GROUP_NAME: &str = "step_group";

/// `cook.prior_outputs(member?)` — the outputs of the preceding
/// output-producing step in the enclosing body (§{lua.prior-outputs},
/// CS-0186). Recorder in `cook-register`, §6.3.2 guard in `cook-execute`.
pub const PRIOR_OUTPUTS_NAME: &str = "prior_outputs";

/// `cook.interactive(cmd, line)` — the register-phase declaration of an
/// interactive unit (§6.3.2). Recorder in `cook-register`, guard in
/// `cook-execute`.
///
/// Deliberately NOT [`ADD_UNIT_INTERACTIVE_FIELD`], which is spelled the same
/// and means something else. The two are a door and a field on a different
/// door's argument; one constant standing for both would tie a rename of
/// either to the other, and the constitution's literal rule cannot tell them
/// apart because a rule that reads text never can.
pub const INTERACTIVE_NAME: &str = "interactive";

/// The `interactive = true` field on a [`ADD_UNIT_NAME`] argument table
/// (§{lua.add-unit}): the legacy single-line interactive shell step, as
/// opposed to a unit declared through [`INTERACTIVE_NAME`].
///
/// A door-FIELD name, written by the Lua that passes the table and read by
/// the Rust that unpacks it — the emitter/consumer pair this module exists
/// for, on a boolean rather than a call.
pub const ADD_UNIT_INTERACTIVE_FIELD: &str = "interactive";

/// `cook.dep_output(name)` — a referenced recipe's terminal outputs as a
/// space-joined string. Phase: Both (§24.7). The register VM resolves and
/// records a DAG edge; the execute VM resolves read-only against the
/// registration snapshot. Two implementations of one door is what §24.7
/// asks for; two spellings of its name is not.
pub const DEP_OUTPUT_NAME: &str = "dep_output";

/// `cook.dep_output_list(name)` — the [`DEP_OUTPUT_NAME`] answer as a Lua
/// sequence. Phase: Both (§24.7).
pub const DEP_OUTPUT_LIST_NAME: &str = "dep_output_list";

/// `cook.dep_output_member(name, member)` — the per-member terminal outputs
/// of a fan-out producer (COOK-96, §22.6). Register-phase only today; named
/// here because `cook-luagen` emits the call and `cook-register` installs it,
/// which is this module's emitter/installer pair.
pub const DEP_OUTPUT_MEMBER_NAME: &str = "dep_output_member";

/// `cook.member_to_string(value)` — a data member's canonical string form
/// (§9.3, COOK-64). Phase: Both, and the ONE door of this group whose whole
/// implementation is shared: `cook_lua_stdlib::install_member_to_string`
/// installs it on both VMs, so the name, the body and the diagnostic have one
/// author. The constant stays here because `cook-luagen` emits the call.
pub const MEMBER_TO_STRING_NAME: &str = "member_to_string";

/// The Lua global the declared-variable surface is reachable under
/// (§{decl.config}, §{lua.var}): `var.NAME`.
///
/// Three ends, and the third is why this is here rather than in
/// `cook-lua-stdlib` with the seal that installs it. Both VMs install the
/// read-only proxy under this name. `cook-register`'s config sandbox exposes
/// the WRITE sink under it, because a `config` body declares values by
/// assigning to `var.NAME`. And [`crate::lua_scan`] scans Lua source for
/// `var.X` reads to compute which declared variables a unit consumed — a
/// determinant of that unit's cache key.
///
/// The scanner is the dangerous end. A rename that reached the two VMs and
/// not the scanner does not fail: it silently records no reads, so a unit
/// stops being keyed on a variable it depends on, and a value change no
/// longer invalidates it. The scanner is pure and lives here, which is as low
/// as the law goes, so this is where the name goes with it.
pub const VAR_GLOBAL_NAME: &str = "var";

/// The optional `discovered_inputs` table on a [`ADD_UNIT_NAME`] argument
/// (§{lua.add-unit-discovered-inputs}): a maker declaring that a unit's real input
/// set is read back from a file the command writes.
///
/// A door-FIELD name, spelled by the Lua that writes it and by the Rust that
/// reads it. Unrelated to the identically-spelled
/// [`crate::cache::cas::artifact_kind::DISCOVERED_INPUTS`], which is an
/// on-disk artifact kind: the two describe different things at different
/// boundaries and are free to diverge. COOK-421 narrowed the constitution's
/// waiver to exactly this half before handing it over, and the point of
/// naming them apart is that the next reader does not have to re-derive that.
pub const ADD_UNIT_DISCOVERED_INPUTS_FIELD: &str = "discovered_inputs";

/// The §24.7 diagnostic for a `cook.dep_output` / `cook.dep_output_list`
/// reference to a name that has no terminal output — either it was never
/// registered, or it registered no `cook` steps.
///
/// One condition, one sentence, both phases. It was written twice before
/// COOK-439 — positionally in `cook-register`, inline in `cook-execute` — and
/// the constitution's duplicate-literal rule could not see the pair, because
/// `"recipe '{}' has …"` and `"recipe '{name}' has …"` are different literals.
/// That is the blind spot the constitution's own gate section warns about,
/// firing on a user-visible sentence: a reader improving one phase's wording
/// would have left the other phase saying something else, and nothing would
/// have failed.
pub fn no_terminal_output_message(name: &str) -> String {
    format!("recipe '{name}' has no terminal output (not registered or has no cook steps)")
}

// ---------------------------------------------------------------------------
// __member_source — one shape, one set of key spellings (COOK-390)
// ---------------------------------------------------------------------------

/// The surface-meta field carrying a member-fanout recipe's data source
/// (`__member_source = { kind = "probe"|"gather", ref = "…" }`, §22.5.10).
pub const MEMBER_SOURCE_FIELD: &str = "__member_source";

/// Key naming the descriptor's kind inside the meta table.
pub const MEMBER_SOURCE_KIND_KEY: &str = "kind";

/// Key naming the probe reference inside the meta table.
pub const MEMBER_SOURCE_REF_KEY: &str = "ref";

/// The retained kind values after command and anonymous-Lua sources were removed.
pub const MEMBER_SOURCE_KIND_PROBE: &str = "probe";
pub const MEMBER_SOURCE_KIND_GATHER: &str = "gather";

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
/// - `Probe { source_ref }` — legacy internal descriptor for `gather <ref>`, the ref verbatim (`key`
///   or `key:field`; probe keys admit any number of segments).
///   Resolution against the probe registry happens in the register
///   pre-pass (COOK-190); the body reads the resolved member array via
///   `cook.probes.get(<verbatim ref>)`.
/// - `Gather { source_ref }` — current `gather <ref>` descriptor. It resolves
///   array probes like `Probe` and also turns a named files manifest's path
///   keys into members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberSourceDescriptor {
    Probe { source_ref: String },
    Gather { source_ref: String },
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
/// This used to say it was "deliberately distinct from
/// `cook_engine::RecipeKind`, the progress-event mirror enum: that one answers
/// 'how should a completed recipe be labelled', this one answers 'what did the
/// author declare'". COOK-421 retired that distinction, because the code never
/// held it: the engine derived its mirror from THIS value
/// (`cook-engine/src/run.rs`) and cook-cli derived the renderer's from that,
/// so the label was the declaration, twice removed, through two hand-written
/// translations that nothing checked. One vocabulary, one declaration.
///
/// The serde spelling is a wire format: `cook-progress` writes this into
/// `.cook/logs` and `cook-logs` reads it back. The coupling runs both ways and
/// is the price of one definition — adding a third recipe flavour here is a
/// change to the on-disk log format, and `cook-progress`'s `wire.rs` schema
/// rules (additive only, or bump `PROGRESS_SCHEMA_VERSION`) apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeKind {
    /// Defaulted so an older reader round-trips a log written without the
    /// field, which is what the renderer's copy of this enum did.
    #[default]
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
    DefaultedLua {
        name: String,
        default_key_name: String,
    },
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
    /// Probes keyed by workspace-qualified probe key, while each `ProbeUnit`
    /// still carries its Cookfile-local declaration key.
    pub probes:
        std::collections::BTreeMap<crate::probe_key::QualifiedProbeKey, crate::ProbeUnit>,
    /// COOK-526: qualified keys of every probe the register pass actually
    /// resolved this invocation — ran `produce`, or resolved with no VM at
    /// all (a synthesised `files`/`tools` declaration; CS-0243 left no third
    /// route, since no probe value is served from a store) — across every
    /// Cookfile in the workspace. The executor's G4 dispatch
    /// consults this before running a probe's `produce` a second time —
    /// see `cook-engine::run`'s `probe_units_by_node` construction — so the
    /// register pre-pass and the executor share one evaluation per key
    /// instead of each independently producing it.
    pub resolved_probe_keys: std::collections::BTreeSet<crate::probe_key::QualifiedProbeKey>,
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

#[cfg(test)]
#[path = "tests/registration_tests.rs"]
mod tests;
