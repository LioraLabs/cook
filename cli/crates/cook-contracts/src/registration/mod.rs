//! Private registration-surface names shared by emitter and installer.
//!
//! Every name here is a wire format: cook-luagen EMITS it into generated
//! Lua, cook-register INSTALLS or READS it on the register VM. None of
//! them fails loudly on drift — a renamed global is simply never found
//! (`if let Ok` lookups silently skip; a `cook.`-table helper resolves to
//! nil and the generated call errors at runtime with no hint of why) — so
//! emitter and consumer MUST import the one constant (COOK-390, modeled
//! on REGISTER_SURFACE_NAME).

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
