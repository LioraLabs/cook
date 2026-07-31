use std::collections::BTreeSet;

use cook_contracts::ACCESSORS;
use cook_lang::ast::Body;

use crate::cook_step::{cook_mode_to_iter_mode, count_to_output_shape, CookMode};
use crate::lua_string::escape_lua_string;
use crate::resolver::{
    BuiltinKind, IterMode, OutputShape, ResolveCtx, ResolveError, Resolved,
};
use crate::sigil;

/// Expand a command string using the unified `$<IDENT>` sigil pipeline.
///
/// CS-0074 / COOK-187 / CS-0122: probe-value references (`$<key:field>` etc.)
/// are handled entirely by the sigil scanner + resolver — no separate
/// `{{...}}` scanner. A sigil whose IDENT contains `:` is dispatched to
/// `Resolved::ProbeRef` by the resolver.
///
/// Returns `(lua_expr, probe_keys_referenced)`.
///
/// Command bodies are ALWAYS a plain Lua string literal or concatenation —
/// never a deferred `function() ... end`. `cook.add_unit` requires a string
/// `command`; a probe-value reference is left as literal `$<key:...>` sigil
/// text inside that string. §22.5.7's register-time capture
/// (`try_expand_probe_templates` in cook-register) detects the sigil text and
/// rewrites the unit's command into an execute-time `cook.probes.get` chunk.
/// Emitting a function-valued command here would silently no-op the unit
/// once `cook.add_unit` coerces the non-string value to `""` — this is the
/// COOK-187 defect; the fix is to never produce that shape.
pub(crate) fn expand_command_template(
    cmd: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
) -> Result<(String, BTreeSet<String>), ResolveError> {
    let spans = sigil::scan(cmd);
    let mut probe_keys: BTreeSet<String> = BTreeSet::new();
    let mut parts: Vec<String> = vec![];
    let mut cursor = 0usize;

    for span in &spans {
        if span.range.start > cursor {
            parts.push(format!("\"{}\"", escape_lua_string(&cmd[cursor..span.range.start])));
        }
        let resolved = crate::resolver::resolve(&span.ident, ctx);
        if let Resolved::ProbeRef { key, .. } = &resolved {
            // COOK-187 / CS-0122: probe-value refs stay LITERAL `$<key:...>`
            // text in the register-time command string. cook.add_unit's
            // CS-0074 capture (§22.5.7) rewrites the string into an
            // execute-time cook.probes.get chunk. Emitting a deferred
            // `function() ... end` here is forbidden — cook.add_unit
            // rejects non-string commands.
            probe_keys.insert(key.clone());
            parts.push(format!("\"{}\"", escape_lua_string(&cmd[span.range.clone()])));
        } else {
            let lua_expr = resolved_to_lua(resolved, &span.ident, consulted_env)?;
            parts.push(lua_expr);
        }
        cursor = span.range.end;
    }
    if cursor < cmd.len() {
        parts.push(format!("\"{}\"", escape_lua_string(&cmd[cursor..])));
    }
    if parts.is_empty() {
        parts.push("\"\"".to_string());
    }
    let concat_expr = if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    };
    Ok((concat_expr, probe_keys))
}

/// How a probe-value reference (`$<key:...>`) lowers during template
/// expansion.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ProbeLowering {
    /// Leave the `$<key:...>` source text in the emitted string literal;
    /// `cook.add_unit`'s register-time capture rewrites it (COOK-187/CS-0122).
    /// Required for anything that feeds `command =`.
    LiteralSigil,
    /// Lower to `tostring(cook.probes.get(...))` evaluated where the
    /// expression is evaluated (register time). Pre-existing behavior for
    /// non-command positions (member-fanout output patterns, test as-names).
    CacheGet,
}

/// Expand a member-fanout body template (§9.3): a cook output pattern, a cook
/// shell command, or a plate/test body. The member sigils `$<in>` and
/// `$<in.FIELD>` bind the current data member (`item`); every other sigil
/// resolves through the normal closed-set [`resolve`] against `ctx` — so
/// `$<out>`, recipe refs, env vars, and probe refs behave exactly as in an
/// ingredient-driven body. `$<in>` is rejected when `ctx.mode` is `OneShot`
/// (a member-fanout body has no path-input source).
///
/// `probe_lowering` selects how a probe-value reference lowers: `LiteralSigil`
/// for anything that feeds a `command =` field (COOK-187/CS-0122 — the string
/// must stay a plain string for `cook.add_unit`'s register-time capture to
/// rewrite), `CacheGet` for non-command positions that resolve at register
/// time (e.g. output patterns, `test` as-names).
///
/// Returns the concatenation expression (unwrapped — the caller decides whether
/// to wrap a command in a deferred `function() return … end` when probe refs
/// are present, for the positions where that pre-existing behavior is kept)
/// together with the set of probe keys it referenced.
pub(crate) fn expand_member_fanout_template(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
    probe_lowering: ProbeLowering,
) -> Result<(String, BTreeSet<String>), ResolveError> {
    let spans = sigil::scan(template);
    let mut parts: Vec<String> = Vec::new();
    let mut probe_keys: BTreeSet<String> = BTreeSet::new();
    let mut last_end = 0usize;

    for span in &spans {
        if span.range.start > last_end {
            parts.push(format!(
                "\"{}\"",
                escape_lua_string(&template[last_end..span.range.start])
            ));
        }
        // §9.3 member binding takes precedence; everything else is the normal
        // closed-set resolution.
        let lua = if let Some(b) = crate::resolver::match_member_sigil(&span.ident) {
            builtin_to_lua(b)
        } else {
            let resolved = crate::resolver::resolve(&span.ident, ctx);
            if let Resolved::ProbeRef { key, .. } = &resolved {
                probe_keys.insert(key.clone());
            }
            // COOK-96 / COOK-221 / CS-0137: $<recipe[in]> inside a fan-out body
            // lowers to a per-member output lookup. `item` is the loop-local Lua
            // variable bound by the fan-out harness (BuiltinKind::Item →
            // cook.member_to_string(item)).
            if let Resolved::ProbeRef { .. } = &resolved {
                if probe_lowering == ProbeLowering::LiteralSigil {
                    // COOK-187 / CS-0122: literal sigil text for register-time
                    // capture — see expand_command_template's doc comment.
                    format!("\"{}\"", escape_lua_string(&template[span.range.clone()]))
                } else {
                    resolved_to_lua(resolved, &span.ident, consulted_env)?
                }
            } else if let Resolved::RecipeMember { ref name } = resolved {
                format!(
                    "cook.dep_output_member(\"{}\", cook.member_to_string(item))",
                    escape_lua_string(name)
                )
            } else {
                resolved_to_lua(resolved, &span.ident, consulted_env)?
            }
        };
        parts.push(lua);
        last_end = span.range.end;
    }

    if last_end < template.len() {
        parts.push(format!("\"{}\"", escape_lua_string(&template[last_end..])));
    }

    let concat = if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    };
    Ok((concat, probe_keys))
}

/// Accumulator for env keys that fall through to `cook.require_var(KEY)` during
/// template expansion. Populated by every expansion path that reaches the
/// "env-runtime" branch. The set is sorted (BTreeSet) so the resulting
/// emitted Lua table is deterministic.
#[derive(Debug, Default, Clone)]
pub struct ConsultedEnv {
    pub keys: BTreeSet<String>,
}

impl ConsultedEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, key: &str) {
        self.keys.insert(key.to_string());
    }

    /// Render as a Lua table literal: `{"A", "B", "C"}`.
    /// Returns `"{}"` for an empty set.
    pub fn to_lua_table(&self) -> String {
        if self.keys.is_empty() {
            return "{}".to_string();
        }
        let parts: Vec<String> = self
            .keys
            .iter()
            .map(|k| format!("\"{}\"", escape_lua_string(k)))
            .collect();
        format!("{{{}}}", parts.join(", "))
    }
}


// ─── New sigil-based substitution engine ────────────────────────────────────

/// Expand a shell-text template using the strict `$<IDENT>` sigil scanner and
/// the closed-set resolver. Returns a Lua expression suitable for inlining as
/// a `cook.add_unit` `command = ...` value.
///
/// Builtins inline as `_cook_in`/`_cook_out`/etc; recipes lower to
/// `cook.dep_output("name")`; env vars lower to `cook.require_var("name")`.
///
/// Returns Err if any placeholder fails to resolve (builtin mode/count violation,
/// or malformed index).
pub(crate) fn expand_sigil_template(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
) -> Result<String, ResolveError> {
    expand_sigil_template_with_chore_params(
        template,
        ctx,
        consulted_env,
        None,
        ProbeLowering::CacheGet,
    )
}

// CS-0128: the quoting law — QCtx, quote_context, quote_for_ctx,
// shell_quote — lives in cook_contracts::quoting (COOK-389). This crate
// CLASSIFIES; cook-register QUOTES; the tag strings in generated Lua are
// QCtx::tag/from_tag, one place, exhaustive on the consumer side.

pub(crate) fn expand_sigil_template_with_chore_params(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
    chore_params: Option<&BTreeSet<String>>,
    probe_lowering: ProbeLowering,
) -> Result<String, ResolveError> {
    let spans = sigil::scan(template);
    if spans.is_empty() {
        // No placeholders — entire string is literal.
        return Ok(format!("\"{}\"", escape_lua_string(template)));
    }

    let mut parts: Vec<String> = Vec::new();
    let mut last_end = 0usize;

    for span in &spans {
        // Emit literal text before this placeholder.
        if span.range.start > last_end {
            let literal = &template[last_end..span.range.start];
            parts.push(format!("\"{}\"", escape_lua_string(literal)));
        }

        // Chore parameters are an innermost binding layer for chore shell
        // steps. Anything not declared as a parameter falls through to the
        // ordinary closed-set resolver, so env and recipe refs behave exactly
        // as they do in recipe bodies.
        let lua_expr = if chore_params.is_some_and(|params| params.contains(&span.ident)) {
            // CS-0128: the sigil expands per its shell quoting context. Scan the
            // command prefix up to this span to classify bare / double / single
            // and thread it into the runtime quoter.
            let ctx =
                cook_contracts::quoting::quote_context(&template[..span.range.start]).tag();
            format!(
                "cook.__quote_param(__cook_params[\"{}\"], \"{}\", \"{}\")",
                escape_lua_string(&span.ident),
                escape_lua_string(&span.ident),
                ctx
            )
        } else {
            let resolved = crate::resolver::resolve(&span.ident, ctx);
            if matches!(resolved, Resolved::ProbeRef { .. })
                && probe_lowering == ProbeLowering::LiteralSigil
            {
                // CS-0193: a chore step's probe ref stays literal `$<...>`
                // text in the captured command — cook.add_unit's scan wires
                // the edge and demand scheduling, and the drain-thread spawn
                // substitutes through the CS-0192 renderer. The old lowering
                // read `cook.probes.get` on the register VM, which raises
                // outside module context and created no edge.
                format!("\"{}\"", escape_lua_string(&template[span.range.clone()]))
            } else {
                resolved_to_lua(resolved, &span.ident, consulted_env)?
            }
        };
        parts.push(lua_expr);

        last_end = span.range.end;
    }

    // Emit any trailing literal text.
    if last_end < template.len() {
        let literal = &template[last_end..];
        parts.push(format!("\"{}\"", escape_lua_string(literal)));
    }

    if parts.is_empty() {
        Ok("\"\"".to_string())
    } else if parts.len() == 1 {
        Ok(parts.into_iter().next().unwrap())
    } else {
        Ok(parts.join(" .. "))
    }
}

/// Convert a `Resolved` value to a Lua expression string.
///
/// `probe_keys` is an optional accumulator for probe keys referenced during
/// expansion; when `Some`, probe-ref keys are inserted into the set so callers
/// can track which probe keys the command depends on.
fn resolved_to_lua(
    resolved: Resolved,
    ident: &str,
    consulted_env: &mut ConsultedEnv,
) -> Result<String, ResolveError> {
    match resolved {
        Resolved::Builtin(b) => Ok(builtin_to_lua(b)),
        Resolved::Recipe { name, accessor } => {
            let escaped = escape_lua_string(&name);
            if let Some(acc) = accessor {
                Ok(format!("path.{}(cook.dep_output(\"{}\"))", acc, escaped))
            } else {
                Ok(format!("cook.dep_output(\"{}\")", escaped))
            }
        }
        Resolved::EnvRuntime(key) => {
            consulted_env.record(&key);
            Ok(format!("cook.require_var(\"{}\")", escape_lua_string(&key)))
        }
        // CS-0195: probe-value reference — one substitution helper, backed by
        // the CS-0192 law over the pre-pass store. Scalars render as their
        // canonical JSON token; composites/null/absent raise register-phase
        // diagnostics instead of interpolating a Lua heap address.
        Resolved::ProbeRef { .. } => {
            Ok(format!("cook.__probe_subst(\"{}\")", escape_lua_string(ident)))
        }
        Resolved::Error(e) => Err(e),
        // COOK-96: $<recipe[in]> is only valid inside a fan-out body (expand_member_fanout_template).
        // Reaching this arm means it appeared in a plain command body where `item` is not in scope.
        Resolved::RecipeMember { name } => Err(ResolveError::RecipeMemberOutsideFanout {
            ident: format!("{}[in]", name),
        }),
    }
}

/// Convert a resolved builtin to its Lua expression.
fn builtin_to_lua(b: BuiltinKind) -> String {
    match b {
        BuiltinKind::In => "_cook_in".to_string(),
        BuiltinKind::InAccessor(acc) => format!("path.{}(_cook_in)", acc),
        BuiltinKind::Out => "_cook_out".to_string(),
        BuiltinKind::OutAccessor(acc) => format!("path.{}(_cook_out)", acc),
        BuiltinKind::OutIndexed(n) => format!("_cook_outs[{}]", n),
        BuiltinKind::OutIndexedAccessor(n, acc) => format!("path.{}(_cook_outs[{}])", acc, n),
        // COOK-63 §9.3: data-member bindings. `$<in>` renders the whole
        // member — canonical key-sorted JSON for a record, the scalar's string
        // form otherwise — via the `cook.member_to_string` runtime helper
        // (provided by the COOK-64 runtime slice). `$<in.FIELD>` reads a
        // record field by key (bracket-indexed so non-identifier keys are safe).
        //
        // COOK-302: the field goes through the same `cook.member_to_string`
        // renderer as the whole member, not `tostring`. A nested field (array
        // or object) is a Lua table, and `tostring` on a table yields an
        // ASLR-fresh address — which lands in the unit's command string and
        // defeats caching forever. §9.3 requires canonical key-sorted JSON for
        // a nested table value, and the bare string form for a scalar.
        BuiltinKind::Item => "cook.member_to_string(item)".to_string(),
        BuiltinKind::ItemField(field) => {
            format!("cook.member_to_string(item[\"{}\"])", escape_lua_string(&field))
        }
    }
}

/// Build a `ResolveCtx` for a cook step shell body.
pub(crate) fn cook_step_ctx<'a>(
    iter_mode: IterMode,
    output_shape: OutputShape,
    recipes_in_scope: &'a BTreeSet<String>,
) -> ResolveCtx<'a> {
    ResolveCtx {
        mode: iter_mode,
        outputs: output_shape,
        recipes_in_scope,
    }
}

// ─── Output pattern analysis (sigil-based) ───────────────────────────────────

/// What drives iteration for an output pattern.
///
/// Classification only. Lowering the pattern to Lua is
/// [`expand_output_pattern`], which is the same call for every variant and is
/// the only half that can fail. Before COOK-357 this enum also carried the
/// dep-driven case's pre-rendered `lua_expr`, which meant classifying a pattern
/// silently lowered it too, on a path with nowhere to report an error.
pub(crate) enum OutputPatternKind {
    /// No iteration source in the pattern — many-to-one.
    Literal,
    /// Pattern contains `$<in>` / `$<in.ACCESSOR>` — one-to-one over own inputs.
    OwnInputAccessor,
    /// Pattern contains `$<recipe.ACCESSOR>` — one-to-one over that dep's outputs.
    DepDriven { dep_name: String },
}

/// Determine the iteration kind of an output pattern WITH full recipe-name knowledge.
pub(crate) fn output_pattern_kind_with_recipes(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    if sigil::scan(pattern)
        .iter()
        .any(|span| crate::resolver::is_own_input_ref(&span.ident))
    {
        return OutputPatternKind::OwnInputAccessor;
    }
    match first_dep_accessor_sigil(pattern, recipe_names) {
        Some(dep_name) => OutputPatternKind::DepDriven { dep_name },
        None => OutputPatternKind::Literal,
    }
}

/// Walk a pattern's `$<TOKEN.SUFFIX>` placeholders and return the first TOKEN
/// that is a recipe in scope carrying a known path accessor.
fn first_dep_accessor_sigil(pattern: &str, recipe_names: &BTreeSet<String>) -> Option<String> {
    sigil::scan(pattern).into_iter().find_map(|span| {
        let (prefix, suffix) = span.ident.rsplit_once('.')?;
        (ACCESSORS.contains(&suffix) && recipe_names.contains(prefix)).then(|| prefix.to_string())
    })
}

/// Expand an output pattern using sigil-based substitution.
///
/// One entry point for every output-pattern kind, dep-driven or not (COOK-357).
/// It previously forked: the dep-driven caller passed the real recipe set while
/// the ordinary caller passed an empty one, so a bare `$<name>` lowered to
/// `cook.dep_output` in one pattern and `cook.require_var` in another purely
/// because the first pattern happened to carry a dep accessor elsewhere.
pub(crate) fn expand_output_pattern(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
    out: &mut ConsultedEnv,
) -> Result<String, ResolveError> {
    // `$<dep.accessor>` normalises to `path.accessor(_cook_in)`: iteration is
    // over the dep's outputs, so `_cook_in` holds the dep output. OneToOne /
    // Single match that step context.
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: recipe_names,
    };

    let spans = sigil::scan(pattern);
    if spans.is_empty() {
        return Ok(format!("\"{}\"", escape_lua_string(pattern)));
    }

    let mut parts: Vec<String> = Vec::new();
    let mut last_end = 0usize;

    for span in &spans {
        if span.range.start > last_end {
            let literal = &pattern[last_end..span.range.start];
            parts.push(format!("\"{}\"", escape_lua_string(literal)));
        }

        parts.push(output_pattern_ident_to_lua(&span.ident, &ctx, out)?);

        last_end = span.range.end;
    }

    if last_end < pattern.len() {
        let literal = &pattern[last_end..];
        parts.push(format!("\"{}\"", escape_lua_string(literal)));
    }

    Ok(if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    })
}

/// Convert an output-pattern sigil ident to a Lua expression.
/// Special case: recipe.accessor in an output pattern expands to path.accessor(_cook_in)
/// (since dep-driven iteration normalizes the dep output to _cook_in).
fn output_pattern_ident_to_lua(
    ident: &str,
    ctx: &ResolveCtx<'_>,
    out: &mut ConsultedEnv,
) -> Result<String, ResolveError> {
    // Check if this is a recipe accessor that should normalize to path.X(_cook_in).
    if let Some(dot_pos) = ident.rfind('.') {
        let prefix = &ident[..dot_pos];
        let suffix = &ident[dot_pos + 1..];
        if ACCESSORS.contains(&suffix) && ctx.recipes_in_scope.contains(prefix) {
            // dep.accessor → path.accessor(_cook_in) (dep-driven normalization)
            return Ok(format!("path.{}(_cook_in)", suffix));
        }
    }

    // Otherwise use normal resolution.
    match crate::resolver::resolve(ident, ctx) {
        Resolved::Builtin(b) => Ok(builtin_to_lua(b)),
        Resolved::Recipe { name, accessor } => {
            let escaped = escape_lua_string(&name);
            Ok(match accessor {
                Some(acc) => format!("path.{}(cook.dep_output(\"{}\"))", acc, escaped),
                None => format!("cook.dep_output(\"{}\")", escaped),
            })
        }
        Resolved::EnvRuntime(key) => {
            out.record(&key);
            Ok(format!("cook.require_var(\"{}\")", escape_lua_string(&key)))
        }
        // CS-0074: probe refs are not expected in output patterns, but if they appear
        // emit the access expression so they aren't silently swallowed.
        // CS-0195: same helper as resolved_to_lua — one renderer per ident.
        Resolved::ProbeRef { .. } => {
            Ok(format!("cook.__probe_subst(\"{}\")", escape_lua_string(ident)))
        }
        // COOK-96: $<recipe[in]> is invalid in an output pattern — output patterns
        // have no fan-out body context and `item` is not in scope.
        Resolved::RecipeMember { name } => Err(ResolveError::RecipeMemberOutsideFanout {
            ident: format!("{}[in]", name),
        }),
        // COOK-357: every resolver error is now a codegen error here. It used to
        // fork — bracket-index errors became `[[SIGIL_ERROR: …]]`
        // string literals, everything else silently fell through to
        // `cook.require_var(ident)` "for backward compat". That fallback turned
        // e.g. `$<out_2>` in a single-output pattern into a lookup for a
        // variable literally named `out_2`, reporting a missing config block
        // instead of the wrong-output-count diagnostic the same sigil gets in a
        // step body.
        Resolved::Error(e) => Err(e),
    }
}

// ─── CS-0024: plate/test mode detection, placeholder validation, body expansion ─

/// CS-0024 §3.4: the iteration mode of a plate/test step body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlateTestMode {
    /// Body references $<in>/$<in.X> (shell) or `input` (Lua), and not the
    /// batched form. One unit per source item.
    OneToOne,
    /// Body references `inputs` (Lua block form only — shell bodies cannot
    /// express batched plate/test since CS-0130 removed `$<all>`), and not
    /// the per-item form. Exactly one unit, full source visible.
    ManyToOne,
    /// Body references neither. Exactly one unit, source not consulted.
    OneShot,
}

#[derive(Debug, thiserror::Error)]
pub enum PlateTestModeError {
    #[error("body contains both per-item and batched references — `{0}` and `{1}` cannot both appear")]
    Mixed(&'static str, &'static str),
}

pub(crate) fn detect_plate_test_mode(body: &Body) -> Result<PlateTestMode, PlateTestModeError> {
    match body {
        Body::ShellBlock(lines) => {
            let joined: String = lines.join("\n");
            // A shell plate/test body is per-item ($<in>) or one-shot. Batched
            // (many-to-one) plate/test is the Lua block form (`inputs`).
            if body_text_has_in_placeholder_sigil(&joined) {
                Ok(PlateTestMode::OneToOne)
            } else {
                Ok(PlateTestMode::OneShot)
            }
        }
        Body::LuaBlock(code) => {
            let has_input = lua_has_free_identifier(code, "input");
            let has_inputs = lua_has_free_identifier(code, "inputs");
            match (has_input, has_inputs) {
                (true, true) => Err(PlateTestModeError::Mixed("input", "inputs")),
                (true, false) => Ok(PlateTestMode::OneToOne),
                (false, true) => Ok(PlateTestMode::ManyToOne),
                (false, false) => Ok(PlateTestMode::OneShot),
            }
        }
    }
}

/// Scan a shell-body text for any `$<in>` or `$<in.ACCESSOR>` placeholder.
fn body_text_has_in_placeholder_sigil(text: &str) -> bool {
    sigil::scan(text)
        .iter()
        .any(|span| crate::resolver::is_own_input_ref(&span.ident))
}

/// Scan a Lua source text for a free-identifier reference to `name`.
///
/// Skips:
/// - text inside `"…"` and `'…'` short strings (with `\` escape rules);
/// - text inside `[[…]]` long strings (any `=` count between brackets);
/// - text inside `--` line comments and `--[[…]]` block comments;
/// - identifier-name positions immediately preceded by `.` or `:` (these
///   are property/method accesses, not free identifiers).
///
/// The scan recognises `name` only as a whole-word identifier, bordered
/// by Lua identifier-character boundaries.
fn lua_has_free_identifier(code: &str, name: &str) -> bool {
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Strings and comments are not code (see `crate::lua_scan`). An
        // unterminated one makes the rest unscannable; report "not found"
        // rather than guessing where it ends.
        match crate::lua_scan::skip_non_code(code, bytes, i) {
            crate::lua_scan::Skip::Ended(next) => {
                i = next;
                continue;
            }
            crate::lua_scan::Skip::Unterminated => return false,
            crate::lua_scan::Skip::Code => {}
        }

        if crate::lua_scan::is_ident_start(bytes[i]) {
            let start = i;
            i = crate::lua_scan::ident_end(bytes, i);
            // `x.name` / `x:name` is a property or method, not a free read.
            let is_field_access =
                start > 0 && (bytes[start - 1] == b'.' || bytes[start - 1] == b':');
            if !is_field_access && &code[start..i] == name {
                return true;
            }
            continue;
        }

        i += 1;
    }
    false
}

// ─── CS-0024: placeholder validator ─────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PlateTestPlaceholderError {
    #[error("`{token}` is not valid in {mode_name} mode (line text: `{line}`)")]
    BadPlaceholder { token: String, mode_name: String, line: String },
    #[error("`{token}` is not valid in a plate or test body — plate and test steps declare no outputs")]
    OutForbidden { token: String },
    #[error("bare path-accessor `$<{accessor}>` is no longer valid; use `$<in.{accessor}>`")]
    BareAccessor { accessor: String },
    #[error("`$<{name}.{accessor}>` is not valid in a plate or test body; plate/test steps have no output pattern")]
    LibAccessor { name: String, accessor: String },
}

pub(crate) fn validate_plate_test_placeholders(
    body: &Body,
    mode: PlateTestMode,
    recipe_names: &BTreeSet<String>,
) -> Result<(), PlateTestPlaceholderError> {
    if let Body::ShellBlock(lines) = body {
        for line in lines {
            for span in sigil::scan(line) {
                validate_sigil_token(&span.ident, mode, line, recipe_names)?;
            }
        }
    }
    Ok(())
}

fn validate_sigil_token(
    ident: &str,
    mode: PlateTestMode,
    line: &str,
    recipe_names: &BTreeSet<String>,
) -> Result<(), PlateTestPlaceholderError> {
    // COOK-357: classification comes from the resolver, so this validator and
    // `expand_plate_test_body` cannot disagree about what an ident IS. It used
    // to re-derive every shape by hand (`ident == "out" || starts_with("out.")
    // || …`), which is how the two drifted: the validator recognised
    // `$<in.ACCESSOR>` by prefix while the expander lowered it blindly, so
    // `$<in.bogus>` passed validation and emitted `path.bogus(_test_in)`.
    //
    // The mode/shape RULES stay here: they are what makes a plate/test body
    // different from a cook body, not a different answer to the same question.
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::None,
        recipes_in_scope: recipe_names,
    };

    // A plate/test step declares no outputs, so every `$<out…>` form is
    // rejected. The resolver reports the count violation; this surfaces the
    // step-kind reason instead, which is the more actionable message.
    if crate::resolver::is_output_ref(ident) {
        return Err(PlateTestPlaceholderError::OutForbidden {
            token: format!("$<{}>", ident),
        });
    }

    // Bare path-accessor: rejected before the resolver would read it as a
    // variable name.
    if ACCESSORS.contains(&ident) {
        return Err(PlateTestPlaceholderError::BareAccessor {
            accessor: ident.to_string(),
        });
    }

    match crate::resolver::resolve(ident, &ctx) {
        // §5.4 firewall: a plate/test step has no output pattern, so it can
        // never name `NAME` as an iteration driver.
        Resolved::Recipe { name, accessor: Some(accessor) } => {
            Err(PlateTestPlaceholderError::LibAccessor { name, accessor })
        }
        // `$<in>` / `$<in.ACCESSOR>` need a per-item source.
        Resolved::Builtin(BuiltinKind::In | BuiltinKind::InAccessor(_))
            if mode != PlateTestMode::OneToOne =>
        {
            Err(PlateTestPlaceholderError::BadPlaceholder {
                token: format!("$<{}>", ident),
                mode_name: format!("{:?}", mode),
                line: line.to_string(),
            })
        }
        // Everything else is either valid or carries a resolver error that
        // `expand_plate_test_body` reports with the recipe and line attached.
        _ => Ok(()),
    }
}

// ─── CS-0024: parametric plate/test body expander ───────────────────────────

/// Expand a plate/test shell body.
///
/// COOK-357: this reaches sigil meaning through [`resolve`] like every other
/// expander. It used to hand-roll its own dispatch chain
/// (`file:` -> `in` -> `in.ACCESSOR` -> recipe -> var), which is why the same
/// ident answered differently here than in a cook body: there was no probe-ref
/// arm, so `$<sys:os>` fell through to the variable path and reported a missing
/// config block; no bracket-index arm, so `$<dep[in]>` did the same; no
/// retired-`env.` arm, so `$<env.HOME>` advised declaring a variable literally
/// named `env.HOME`; and `in.ACCESSOR` was not checked against the accessor
/// set, so `$<in.bogus>` emitted `path.bogus(...)` and surfaced as a raw Lua
/// "attempt to call a nil value".
///
/// The mechanism that legitimately differs stays here: `$<in>` binds the
/// caller's `iter_var` (a plate/test unit has no `_cook_in`), and a plate/test
/// step declares no outputs, so `$<out>` is rejected upstream by
/// [`validate_plate_test_placeholders`].
pub(crate) fn expand_plate_test_body(
    template: &str,
    recipe_names: &BTreeSet<String>,
    iter_var: &str,
    out: &mut ConsultedEnv,
) -> Result<(String, BTreeSet<String>), ResolveError> {
    let spans = sigil::scan(template);
    if spans.is_empty() {
        return Ok((format!("\"{}\"", escape_lua_string(template)), BTreeSet::new()));
    }

    // A plate/test body iterates one-to-one over its source and declares no
    // outputs. `OneShot` bodies bind `iter_var` to `""` at the call site rather
    // than changing the mode, so `$<in>` keeps one meaning here.
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::None,
        recipes_in_scope: recipe_names,
    };

    let mut parts: Vec<String> = Vec::new();
    let mut probe_keys: BTreeSet<String> = BTreeSet::new();
    let mut last_end = 0usize;

    for span in &spans {
        if span.range.start > last_end {
            let literal = &template[last_end..span.range.start];
            parts.push(format!("\"{}\"", escape_lua_string(literal)));
        }

        let lua = match crate::resolver::resolve(&span.ident, &ctx) {
            // The one substitution that is genuinely plate/test-specific: the
            // iteration variable is the caller's, not `_cook_in`.
            Resolved::Builtin(BuiltinKind::In) => iter_var.to_string(),
            Resolved::Builtin(BuiltinKind::InAccessor(acc)) => {
                format!("path.{}({})", acc, iter_var)
            }
            // Collected, not lowered on the caller's behalf: a test command has
            // no execute-phase probe substitution, so the caller rejects the
            // step. See `test_step::reject_probe_refs_in_command`.
            Resolved::ProbeRef { ref key } => {
                probe_keys.insert(key.clone());
                // CS-0195: same helper; the caller rejects test-position probe
                // refs before this string is ever used.
                format!("cook.__probe_subst(\"{}\")", escape_lua_string(&span.ident))
            }
            other => resolved_to_lua(other, &span.ident, out)?,
        };
        parts.push(lua);

        last_end = span.range.end;
    }

    if last_end < template.len() {
        let literal = &template[last_end..];
        parts.push(format!("\"{}\"", escape_lua_string(literal)));
    }

    let concat = if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    };
    Ok((concat, probe_keys))
}

// ─── cook step shell body expansion (sigil-based) ─────────────────────────

/// Expand a shell command template using sigil substitution,
/// recording consulted env keys into `out`.
/// Used for cook-step `using { ... }` bodies.

// ─── Validation helpers (sigil-based) ───────────────────────────────────────

/// Validation context for `validate_placeholders`.
pub(crate) struct PlaceholderValidationContext<'a> {
    pub mode: &'a CookMode,
    pub declared_output_count: usize,
    pub recipe_names: &'a BTreeSet<String>,
}

/// Validate all `$<...>` placeholders in `body_text` against the given context.
/// Returns `Err(diagnostic)` on the first violation.
pub(crate) fn validate_placeholders(
    body_text: &str,
    ctx: &PlaceholderValidationContext,
) -> Result<(), String> {
    let resolver_mode = cook_mode_to_iter_mode(ctx.mode);
    let output_shape = count_to_output_shape(ctx.declared_output_count);
    let rctx = ResolveCtx {
        mode: resolver_mode,
        outputs: output_shape,
        recipes_in_scope: ctx.recipe_names,
    };

    for span in sigil::scan(body_text) {
        let resolved = crate::resolver::resolve(&span.ident, &rctx);
        if let Resolved::Error(e) = resolved {
            return Err(e.to_string());
        }
        // CS-0101: a well-formed `$<file:PATH>` is accepted in any cook-step
        // body mode; skip the accessor shape check below (a `file:` ident can
        // never be a `recipe.accessor` form). Bad paths were already rejected
        // by the resolver Error branch above.
        if span.ident.starts_with("file:") {
            continue;
        }
        // Additionally check lib.accessor in a cook-step body (rejected by CS-0022 §6.7).
        if let Some(dot) = span.ident.rfind('.') {
            let prefix = &span.ident[..dot];
            let suffix = &span.ident[dot + 1..];
            if ACCESSORS.contains(&suffix) && ctx.recipe_names.contains(prefix) {
                return Err(format!(
                    "$<{}.{}> is rejected inside a cook-step body; \
                     use $<in.{}> if `{}` is the driver, or reach for Lua otherwise",
                    prefix, suffix, suffix, prefix
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/template_tests.rs"]
mod tests;

// ─── sigil_template_tests module ─────────────────────────────────────────────
#[cfg(test)]
#[path = "tests/sigil_template_tests.rs"]
mod sigil_template_tests;

// ─── CS-0074: probe-value placeholder tests (unified $<...> sigil pipeline) ──
#[cfg(test)]
#[path = "tests/probe_template_tests.rs"]
mod probe_template_tests;
