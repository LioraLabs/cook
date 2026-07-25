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
    file_refs: &mut FileRefs,
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
            let lua_expr = resolved_to_lua(resolved, &span.ident, consulted_env, file_refs)?;
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
    /// non-command positions (for_each output patterns, test as-names).
    CacheGet,
}

/// Expand a `for_each` body template (§8.3): a cook output pattern, a cook
/// shell command, or a plate/test body. The member sigils `$<in>` and
/// `$<in.FIELD>` bind the current data member (`item`); every other sigil
/// resolves through the normal closed-set [`resolve`] against `ctx` — so
/// `$<out>`, recipe refs, env vars, and probe refs behave exactly as in an
/// ingredient-driven body. `$<in>` is rejected when `ctx.mode` is `OneShot`
/// (a `for_each` body has no path-input source).
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
pub(crate) fn expand_for_each_template(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
    file_refs: &mut FileRefs,
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
        // §8.3 member binding takes precedence; everything else is the normal
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
                    resolved_to_lua(resolved, &span.ident, consulted_env, file_refs)?
                }
            } else if let Resolved::RecipeMember { ref name } = resolved {
                format!(
                    "cook.dep_output_member(\"{}\", cook.member_to_string(item))",
                    escape_lua_string(name)
                )
            } else {
                resolved_to_lua(resolved, &span.ident, consulted_env, file_refs)?
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

/// CS-0101 accumulator for `$<file:PATTERN>` references seen during template
/// expansion. Patterns are kept in first-appearance order (deduped); each is
/// lowered to a register-time hoisted local `_cook_fr_<tag>_<n>` so the
/// substitution value is computed at register time even when the surrounding
/// command is wrapped in a probe-deferred `function() return ... end` (still
/// true for `test`/`plate` bodies; a native `cook`-step command never wraps —
/// see COOK-187/CS-0122 in `expand_command_template`'s doc comment).
#[derive(Debug, Clone)]
pub(crate) struct FileRefs {
    tag: String,
    pub patterns: Vec<String>,
}

impl FileRefs {
    pub fn new(tag: impl Into<String>) -> Self {
        Self { tag: tag.into(), patterns: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Record `pattern` (idempotent) and return the Lua local that will hold
    /// its space-joined resolution.
    pub fn local_for(&mut self, pattern: &str) -> String {
        let idx = match self.patterns.iter().position(|p| p == pattern) {
            Some(i) => i,
            None => {
                self.patterns.push(pattern.to_string());
                self.patterns.len() - 1
            }
        };
        format!("_cook_fr_{}_{}", self.tag, idx + 1)
    }

    /// `local _cook_fr_T_1 = cook.file_ref("p1")` … one line per pattern.
    pub fn hoist_lines(&self, indent: &str) -> String {
        self.patterns
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "{}local _cook_fr_{}_{} = cook.file_ref(\"{}\")\n",
                    indent,
                    self.tag,
                    i + 1,
                    escape_lua_string(p)
                )
            })
            .collect()
    }

    /// `{"p1", "p2"}` — the `file_refs` field for cook.add_unit.
    pub fn to_lua_table(&self) -> String {
        let items: Vec<String> = self
            .patterns
            .iter()
            .map(|p| format!("\"{}\"", escape_lua_string(p)))
            .collect();
        format!("{{{}}}", items.join(", "))
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
    file_refs: &mut FileRefs,
) -> Result<String, ResolveError> {
    expand_sigil_template_with_chore_params(template, ctx, consulted_env, file_refs, None)
}

/// CS-0128: the shell quoting context a sigil span sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QCtx {
    /// Outside any quote — the value must be single-quoted for word-safety.
    Bare,
    /// Inside a double-quoted region — the value is escaped for that context.
    Double,
    /// Inside a single-quoted region — the value is emitted verbatim.
    Single,
}

/// Classify the shell quoting context at the end of `prefix` by scanning the
/// command text left-to-right, tracking single/double quote state and
/// backslash escapes (POSIX: backslash is inert inside single quotes).
fn quote_context(prefix: &str) -> QCtx {
    let (mut sq, mut dq, mut esc) = (false, false, false);
    for c in prefix.chars() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if !sq => esc = true,
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            _ => {}
        }
    }
    if sq {
        QCtx::Single
    } else if dq {
        QCtx::Double
    } else {
        QCtx::Bare
    }
}

pub(crate) fn expand_sigil_template_with_chore_params(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
    file_refs: &mut FileRefs,
    chore_params: Option<&BTreeSet<String>>,
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
            let ctx = match quote_context(&template[..span.range.start]) {
                QCtx::Bare => "bare",
                QCtx::Double => "dquote",
                QCtx::Single => "squote",
            };
            format!(
                "cook.__quote_param(__cook_params[\"{}\"], \"{}\", \"{}\")",
                escape_lua_string(&span.ident),
                escape_lua_string(&span.ident),
                ctx
            )
        } else {
            let resolved = crate::resolver::resolve(&span.ident, ctx);
            resolved_to_lua(resolved, &span.ident, consulted_env, file_refs)?
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
    _ident: &str,
    consulted_env: &mut ConsultedEnv,
    file_refs: &mut FileRefs,
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
        // CS-0074: probe-value reference — emit a tostring-wrapped cache read.
        // The access expression is pre-built by the resolver.
        Resolved::ProbeRef { access, .. } => Ok(format!("tostring({})", access)),
        // CS-0101: `$<file:PATH>` — lower to the hoisted register-time local
        // holding the space-joined match list (see [`FileRefs`]).
        Resolved::FileRef { pattern } => Ok(file_refs.local_for(&pattern)),
        Resolved::Error(e) => Err(e),
        // COOK-96: $<recipe[in]> is only valid inside a fan-out body (expand_for_each_template).
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
        // COOK-63 §8.3: data-member bindings. `$<in>` renders the whole
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

/// Result of analyzing an output pattern for dep-driven or own-input iteration.
pub(crate) enum OutputPatternKind {
    /// Pattern is fully literal — many-to-one (no iteration source in output).
    Literal,
    /// Pattern contains `$<in.ACCESSOR>` — one-to-one over own inputs.
    OwnInputAccessor,
    /// Pattern contains `$<recipe.ACCESSOR>` — one-to-one over that dep's outputs.
    DepDriven { dep_name: String, lua_expr: String },
}

/// Determine the iteration kind of an output pattern WITH full recipe-name knowledge.
pub(crate) fn output_pattern_kind_with_recipes(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    let spans = sigil::scan(pattern);
    for span in &spans {
        let ident = &span.ident;
        if ident == "in" || ident.starts_with("in.") {
            return OutputPatternKind::OwnInputAccessor;
        }
    }
    // Check for dep-driven (recipe.accessor) in output pattern.
    if let Some((dep, accessor)) = first_dep_accessor_sigil(pattern, recipe_names) {
        let mut _discard = ConsultedEnv::new();
        let lua_expr = expand_dep_driven_output_pattern(&dep, &accessor, pattern, recipe_names, &mut _discard);
        return OutputPatternKind::DepDriven { dep_name: dep, lua_expr };
    }
    OutputPatternKind::Literal
}

/// Analyze an output pattern (alias kept for callers using the old name).
pub(crate) fn analyze_output_pattern(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    output_pattern_kind_with_recipes(pattern, recipe_names)
}

/// Walk a pattern's `$<TOKEN.SUFFIX>` placeholders and return the first whose
/// TOKEN is in `recipe_names` and SUFFIX is a known path accessor.
fn first_dep_accessor_sigil(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> Option<(String, String)> {
    for span in sigil::scan(pattern) {
        let ident = &span.ident;
        if let Some(dot_pos) = ident.rfind('.') {
            let prefix = &ident[..dot_pos];
            let suffix = &ident[dot_pos + 1..];
            if ACCESSORS.contains(&suffix) && recipe_names.contains(prefix) {
                return Some((prefix.to_string(), suffix.to_string()));
            }
        }
    }
    None
}

/// Expand a dep-driven output pattern (e.g. `build/$<protos.stem>.o`) into a Lua expression.
/// Normalizes `$<dep.accessor>` → `path.accessor(_cook_in)` by substituting
/// via the sigil path with the dep-driven context.
pub(crate) fn expand_dep_driven_output_pattern(
    _dep_name: &str,
    _accessor: &str,
    pattern: &str,
    recipe_names: &BTreeSet<String>,
    out: &mut ConsultedEnv,
) -> String {
    // In a dep-driven output pattern, $<dep.accessor> expands to path.accessor(_cook_in)
    // because iteration is over the dep's outputs (so _cook_in holds the dep output).
    // We use OneToOne mode and Single output shape to match the step context.
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: recipe_names,
    };
    expand_sigil_output_pattern(pattern, &ctx, out)
}

/// Expand an output pattern using sigil-based substitution.
/// In output patterns, `$<dep.accessor>` where dep is a recipe expands to
/// `path.accessor(_cook_in)` (dep-driven iteration normalizes to own-input).
pub(crate) fn expand_output_pattern(pattern: &str, out: &mut ConsultedEnv) -> String {
    // Output patterns only admit: $<in>, $<in.accessor> → own-input iteration
    // Everything else is an env var or literal (output patterns don't reference recipes
    // via their accessor in the body — that goes through dep_name → lua_expr).
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: &BTreeSet::new(),
    };
    expand_sigil_output_pattern(pattern, &ctx, out)
}

/// Expand an output pattern with sigil substitution in dep-driven context.
/// `$<dep.acc>` → `path.acc(_cook_in)` when dep is in scope as a recipe.
fn expand_sigil_output_pattern(
    pattern: &str,
    ctx: &ResolveCtx<'_>,
    out: &mut ConsultedEnv,
) -> String {
    let spans = sigil::scan(pattern);
    if spans.is_empty() {
        return format!("\"{}\"", escape_lua_string(pattern));
    }

    let mut parts: Vec<String> = Vec::new();
    let mut last_end = 0usize;

    for span in &spans {
        if span.range.start > last_end {
            let literal = &pattern[last_end..span.range.start];
            parts.push(format!("\"{}\"", escape_lua_string(literal)));
        }

        // In output patterns, dep.accessor references should expand to path.accessor(_cook_in)
        // because this is where dep-driven iteration is declared.
        let ident = &span.ident;
        let lua_expr = output_pattern_ident_to_lua(ident, ctx, out);
        parts.push(lua_expr);

        last_end = span.range.end;
    }

    if last_end < pattern.len() {
        let literal = &pattern[last_end..];
        parts.push(format!("\"{}\"", escape_lua_string(literal)));
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}

/// Convert an output-pattern sigil ident to a Lua expression.
/// Special case: recipe.accessor in an output pattern expands to path.accessor(_cook_in)
/// (since dep-driven iteration normalizes the dep output to _cook_in).
fn output_pattern_ident_to_lua(
    ident: &str,
    ctx: &ResolveCtx<'_>,
    out: &mut ConsultedEnv,
) -> String {
    // Check if this is a recipe accessor that should normalize to path.X(_cook_in).
    if let Some(dot_pos) = ident.rfind('.') {
        let prefix = &ident[..dot_pos];
        let suffix = &ident[dot_pos + 1..];
        if ACCESSORS.contains(&suffix) && ctx.recipes_in_scope.contains(prefix) {
            // dep.accessor → path.accessor(_cook_in) (dep-driven normalization)
            return format!("path.{}(_cook_in)", suffix);
        }
    }

    // Otherwise use normal resolution.
    let resolved = crate::resolver::resolve(ident, ctx);
    match resolved {
        Resolved::Builtin(b) => builtin_to_lua(b),
        Resolved::Recipe { name, accessor } => {
            let escaped = escape_lua_string(&name);
            if let Some(acc) = accessor {
                format!("path.{}(cook.dep_output(\"{}\"))", acc, escaped)
            } else {
                format!("cook.dep_output(\"{}\")", escaped)
            }
        }
        Resolved::EnvRuntime(key) => {
            out.record(&key);
            format!("cook.require_var(\"{}\")", escape_lua_string(&key))
        }
        // CS-0074: probe refs are not expected in output patterns, but if they appear
        // emit the access expression so they aren't silently swallowed.
        Resolved::ProbeRef { access, .. } => format!("tostring({})", access),
        // CS-0101: a file reference is an input, not an iteration driver — it is
        // invalid in a cook output pattern. The checked codegen path rejects it
        // up front (`check_output_pattern_no_bare_accessors`); this sentinel
        // covers the unchecked `generate` path, mirroring RecipeMember below.
        Resolved::FileRef { pattern } => {
            let e = ResolveError::FileRefInOutputPattern {
                ident: format!("file:{}", pattern),
            };
            format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string()))
        }
        // CS-0101: a malformed file-ref path is a hard error, not an env-var
        // fallback — keep the typed diagnostic visible.
        Resolved::Error(e @ ResolveError::FileRefBadPath { .. }) => {
            format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string()))
        }
        // COOK-221 / CS-0137: bracket-index diagnostics are hard errors, not
        // env-var fallbacks — keep the typed diagnostic visible (mirrors
        // FileRefBadPath above).
        Resolved::Error(
            e @ (ResolveError::RecipeMemberEmptyIndex { .. }
            | ResolveError::RecipeMemberBadIndex { .. }
            | ResolveError::RecipeMemberUnknownRecipe { .. }),
        ) => {
            format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string()))
        }
        Resolved::Error(_) => {
            // In output patterns, errors fall through to env lookup for backward compat
            // with patterns that use $<TOKEN> where TOKEN is an env var name.
            out.record(ident);
            format!("cook.require_var(\"{}\")", escape_lua_string(ident))
        }
        // COOK-96: $<recipe[in]> is invalid in an output pattern — output patterns have no
        // fan-out body context and `item` is not in scope. Emit a sentinel string that
        // surfaces as a Lua load-time string so the error is visible without a runtime panic.
        Resolved::RecipeMember { name } => {
            // Single-escape via the typed error's Display, matching every other
            // SIGIL_ERROR site (recipe.rs, cook_step.rs, …) and keeping the prose
            // identical to the typed error `resolved_to_lua` returns for the same case.
            let e = ResolveError::RecipeMemberOutsideFanout {
                ident: format!("{}[in]", name),
            };
            format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string()))
        }
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
    for span in sigil::scan(text) {
        let ident = &span.ident;
        if ident == "in" || ident.starts_with("in.") {
            return true;
        }
    }
    false
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
        let b = bytes[i];

        // Skip line comments: `-- … <newline>`.
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            // Long comment: `--[[ … ]]` or `--[==[ … ]==]`.
            if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                let (eq_count, after_open) = count_long_bracket_eqs(&bytes[i + 3..]);
                if let Some(after_open_pos) = after_open {
                    let close_marker = format!("]{}]", "=".repeat(eq_count));
                    if let Some(rel) = code[i + 3 + after_open_pos..].find(&close_marker) {
                        i = i + 3 + after_open_pos + rel + close_marker.len();
                        continue;
                    } else {
                        return false; // unterminated — treat as unscannable
                    }
                }
            }
            // Line comment.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Skip short strings.
        if b == b'"' || b == b'\'' {
            let quote = b;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            i += 1; // skip closing quote
            continue;
        }

        // Skip long strings: `[[ … ]]` or `[==[ … ]==]`.
        if b == b'[' {
            let (eq_count, after_open) = count_long_bracket_eqs(&bytes[i + 1..]);
            if let Some(after_open_pos) = after_open {
                let close_marker = format!("]{}]", "=".repeat(eq_count));
                if let Some(rel) = code[i + 1 + after_open_pos..].find(&close_marker) {
                    i = i + 1 + after_open_pos + rel + close_marker.len();
                    continue;
                } else {
                    return false;
                }
            }
        }

        // Identifier match.
        if is_lua_ident_start(b) {
            let ident_start = i;
            while i < bytes.len() && is_lua_ident_cont(bytes[i]) {
                i += 1;
            }
            let before_is_field_access = ident_start > 0
                && (bytes[ident_start - 1] == b'.' || bytes[ident_start - 1] == b':');
            if !before_is_field_access && &code[ident_start..i] == name {
                return true;
            }
            continue;
        }

        i += 1;
    }
    false
}

/// Helper: at byte position `bytes[0]` we're past the leading `[`. If the
/// next chars are `=*[`, we have a long-bracket open. Returns
/// (equality count, byte offset just past the second `[`).
fn count_long_bracket_eqs(bytes: &[u8]) -> (usize, Option<usize>) {
    let mut eq = 0;
    while eq < bytes.len() && bytes[eq] == b'=' {
        eq += 1;
    }
    if eq < bytes.len() && bytes[eq] == b'[' {
        (eq, Some(eq + 1))
    } else {
        (0, None)
    }
}

fn is_lua_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_lua_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
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
    // CS-0101: `$<file:PATH>` is valid in any plate/test body mode (it is a
    // pure substitution, independent of the iteration source). Accepted before
    // the shape checks below; a malformed path surfaces from the expansion
    // path as a SIGIL_ERROR sentinel.
    if ident.starts_with("file:") {
        return Ok(());
    }

    // $<out>, $<out_N>, $<out.X>, $<out_N.X>: all rejected.
    if ident == "out"
        || ident.starts_with("out.")
        || (ident.starts_with("out_") && ident[4..].chars().next().map_or(false, |c| c.is_ascii_digit()))
    {
        return Err(PlateTestPlaceholderError::OutForbidden {
            token: format!("$<{}>", ident),
        });
    }

    // $<in> or $<in.X>: must be in OneToOne.
    if ident == "in" || ident.starts_with("in.") {
        if mode != PlateTestMode::OneToOne {
            return Err(PlateTestPlaceholderError::BadPlaceholder {
                token: format!("$<{}>", ident),
                mode_name: format!("{:?}", mode),
                line: line.to_string(),
            });
        }
        return Ok(());
    }

    // Bare path-accessor: rejected.
    if matches!(ident, "stem" | "name" | "ext" | "dir") {
        return Err(PlateTestPlaceholderError::BareAccessor {
            accessor: ident.to_string(),
        });
    }

    // $<NAME.ACCESSOR> where NAME is a recipe in scope: rejected (§5.4 firewall).
    if let Some((prefix, suffix)) = ident.rsplit_once('.') {
        if recipe_names.contains(prefix) && matches!(suffix, "stem" | "name" | "ext" | "dir") {
            return Err(PlateTestPlaceholderError::LibAccessor {
                name: prefix.to_string(),
                accessor: suffix.to_string(),
            });
        }
    }

    Ok(())
}

// ─── CS-0024: parametric plate/test body expander ───────────────────────────

/// Plate/test variant: substitute `$<in>` to `iter_var` (a shell body is
/// per-item or one-shot only — see [`PlateTestMode`]), and reject `$<out>` /
/// `$<out_N>` (use `validate_plate_test_placeholders` before calling).
/// `$<NAME>` resolves to `cook.dep_output(NAME)` if `NAME`
/// is a recipe; otherwise to `cook.require_var(NAME)`.
pub(crate) fn expand_plate_test_body(
    template: &str,
    recipe_names: &BTreeSet<String>,
    iter_var: &str,
    out: &mut ConsultedEnv,
    file_refs: &mut FileRefs,
) -> String {
    let spans = sigil::scan(template);
    if spans.is_empty() {
        return format!("\"{}\"", escape_lua_string(template));
    }

    let mut parts: Vec<String> = Vec::new();
    let mut last_end = 0usize;

    for span in &spans {
        if span.range.start > last_end {
            let literal = &template[last_end..span.range.start];
            parts.push(format!("\"{}\"", escape_lua_string(literal)));
        }

        let ident = &span.ident;
        // CS-0101: `file:` dispatch first (a plate/test body is substitution-
        // only — no `file_refs` unit field — but the hoisted local still
        // substitutes). A bad path lowers to the SIGIL_ERROR sentinel,
        // matching the other non-Result expansion surfaces.
        let lua = if let Some(resolved) = crate::resolver::match_file_ref(ident) {
            match resolved {
                Resolved::FileRef { pattern } => file_refs.local_for(&pattern),
                Resolved::Error(e) => {
                    format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string()))
                }
                _ => unreachable!("match_file_ref returns FileRef or Error only"),
            }
        } else if ident == "in" {
            iter_var.to_string()
        } else if let Some(acc) = ident.strip_prefix("in.") {
            format!("path.{}({})", acc, iter_var)
        } else if recipe_names.contains(ident.as_str()) {
            format!("cook.dep_output(\"{}\")", escape_lua_string(ident))
        } else {
            // CS-0172: `var.` is the explicit disambiguating prefix.
            let key = ident.strip_prefix("var.").unwrap_or(ident);
            out.record(key);
            format!("cook.require_var(\"{}\")", escape_lua_string(key))
        };
        parts.push(lua);

        last_end = span.range.end;
    }

    if last_end < template.len() {
        let literal = &template[last_end..];
        parts.push(format!("\"{}\"", escape_lua_string(literal)));
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
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
