use std::collections::BTreeSet;

use cook_lang::ast::Body;

use crate::cook_step::CookMode;
use crate::lua_string::escape_lua_string;
use crate::resolver::{
    BuiltinKind, IterMode, OutputShape, ResolveCtx, ResolveError, Resolved,
};
use crate::sigil;

/// Accumulator for env keys that fall through to `cook.require_env(KEY)` during
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

/// Known accessor suffixes for dep-driven iteration patterns.
const DEP_ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

// ─── New sigil-based substitution engine ────────────────────────────────────

/// Expand a shell-text template using the strict `$<IDENT>` sigil scanner and
/// the closed-set resolver. Returns a Lua expression suitable for inlining as
/// a `cook.add_unit` `command = ...` value.
///
/// Builtins inline as `_cook_in`/`_cook_out`/etc; recipes lower to
/// `cook.dep_output("name")`; env vars lower to `cook.require_env("name")`.
///
/// Returns Err if any placeholder fails to resolve (builtin mode/count violation,
/// or malformed index).
pub(crate) fn expand_sigil_template(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
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

        // Resolve the placeholder.
        let resolved = crate::resolver::resolve(&span.ident, ctx);
        let lua_expr = resolved_to_lua(resolved, &span.ident, consulted_env)?;
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
fn resolved_to_lua(
    resolved: Resolved,
    _ident: &str,
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
            Ok(format!("cook.require_env(\"{}\")", escape_lua_string(&key)))
        }
        Resolved::Error(e) => Err(e),
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
        BuiltinKind::All => "_cook_all".to_string(),
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
    #[allow(dead_code)]
    DepDriven { dep_name: String, accessor: String, lua_expr: String },
}

/// Determine the iteration kind of an output pattern WITHOUT recipe-name knowledge.
#[allow(dead_code)]
pub(crate) fn output_pattern_kind(pattern: &str) -> OutputPatternKind {
    let spans = sigil::scan(pattern);
    for span in &spans {
        let ident = &span.ident;
        if ident == "in" || ident.starts_with("in.") {
            return OutputPatternKind::OwnInputAccessor;
        }
    }
    OutputPatternKind::Literal
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
        return OutputPatternKind::DepDriven { dep_name: dep, accessor, lua_expr };
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
            if DEP_ACCESSORS.contains(&suffix) && recipe_names.contains(prefix) {
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
        if DEP_ACCESSORS.contains(&suffix) && ctx.recipes_in_scope.contains(prefix) {
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
            format!("cook.require_env(\"{}\")", escape_lua_string(&key))
        }
        Resolved::Error(_) => {
            // In output patterns, errors fall through to env lookup for backward compat
            // with patterns that use $<TOKEN> where TOKEN is an env var name.
            out.record(ident);
            format!("cook.require_env(\"{}\")", escape_lua_string(ident))
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
    /// Body references $<all> (shell) or `inputs` (Lua), and not the
    /// per-item form. Exactly one unit, full source visible.
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
            let has_in = body_text_has_in_placeholder_sigil(&joined);
            let has_all = body_text_has_sigil_token(&joined, "all");
            match (has_in, has_all) {
                (true, true) => Err(PlateTestModeError::Mixed("$<in>", "$<all>")),
                (true, false) => Ok(PlateTestMode::OneToOne),
                (false, true) => Ok(PlateTestMode::ManyToOne),
                (false, false) => Ok(PlateTestMode::OneShot),
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

/// Scan a shell-body text for `$<TOKEN>` with a specific ident.
fn body_text_has_sigil_token(text: &str, token: &str) -> bool {
    for span in sigil::scan(text) {
        if span.ident == token {
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
    #[error("`$<{name}.{accessor}>` is not valid in a plate or test body (the §5.4 firewall applies — plate/test have no output pattern)")]
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

    // $<all>: must be in ManyToOne.
    if ident == "all" {
        if mode != PlateTestMode::ManyToOne {
            return Err(PlateTestPlaceholderError::BadPlaceholder {
                token: "$<all>".to_string(),
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

/// Plate/test variant: substitute `$<in>` to `iter_var`, `$<all>` to `all_var`,
/// and reject `$<out>` / `$<out_N>` (use `validate_plate_test_placeholders`
/// before calling). `$<NAME>` resolves to `cook.dep_output(NAME)` if `NAME`
/// is a recipe; otherwise to `cook.require_env(NAME)`.
pub(crate) fn expand_plate_test_body(
    template: &str,
    recipe_names: &BTreeSet<String>,
    iter_var: &str,
    all_var: &str,
    out: &mut ConsultedEnv,
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
        let lua = if ident == "in" {
            iter_var.to_string()
        } else if let Some(acc) = ident.strip_prefix("in.") {
            format!("path.{}({})", acc, iter_var)
        } else if ident == "all" {
            format!("table.concat({}, \" \")", all_var)
        } else if recipe_names.contains(ident.as_str()) {
            format!("cook.dep_output(\"{}\")", escape_lua_string(ident))
        } else {
            // Strip env. prefix if present.
            let key = ident.strip_prefix("env.").unwrap_or(ident);
            out.record(key);
            format!("cook.require_env(\"{}\")", escape_lua_string(key))
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
pub(crate) fn expand_template_to_lua_with_deps_tracked(
    template: &str,
    recipe_names: &BTreeSet<String>,
    iter_mode: IterMode,
    output_shape: OutputShape,
    out: &mut ConsultedEnv,
) -> String {
    let ctx = cook_step_ctx(iter_mode, output_shape, recipe_names);
    match expand_sigil_template(template, &ctx, out) {
        Ok(s) => s,
        Err(e) => {
            // Emit an error sentinel (codegen validation should have caught this).
            format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string()))
        }
    }
}

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
    let output_shape = count_to_shape(ctx.declared_output_count);
    let rctx = ResolveCtx {
        mode: resolver_mode,
        outputs: output_shape,
        recipes_in_scope: ctx.recipe_names,
    };

    for span in sigil::scan(body_text) {
        let resolved = crate::resolver::resolve(&span.ident, &rctx);
        if let Resolved::Error(e) = resolved {
            return Err(format!("CS-0022: {}", e));
        }
        // Additionally check lib.accessor in using body (rejected by CS-0022 §6.7).
        if let Some(dot) = span.ident.rfind('.') {
            let prefix = &span.ident[..dot];
            let suffix = &span.ident[dot + 1..];
            if DEP_ACCESSORS.contains(&suffix) && ctx.recipe_names.contains(prefix) {
                return Err(format!(
                    "CS-0022: $<{}.{}> is rejected inside using-clause body; \
                     use $<in.{}> if `{}` is the driver, or reach for Lua otherwise",
                    prefix, suffix, suffix, prefix
                ));
            }
        }
    }
    Ok(())
}

fn cook_mode_to_iter_mode(mode: &CookMode) -> IterMode {
    match mode {
        CookMode::OneToOne | CookMode::OneToMany => IterMode::OneToOne,
        CookMode::ManyToOne | CookMode::BlockStep => IterMode::ManyToOne,
        CookMode::DeclarationOnly => IterMode::OneShot,
    }
}

fn count_to_shape(n: usize) -> OutputShape {
    match n {
        0 => OutputShape::None,
        1 => OutputShape::Single,
        n => OutputShape::Multi(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ConsultedEnv tests ───────────────────────────────────────────────────

    #[test]
    fn consulted_env_to_lua_table_empty() {
        let c = ConsultedEnv::new();
        assert_eq!(c.to_lua_table(), "{}");
    }

    #[test]
    fn consulted_env_to_lua_table_sorted() {
        let mut c = ConsultedEnv::new();
        c.record("Z");
        c.record("A");
        c.record("M");
        assert_eq!(c.to_lua_table(), "{\"A\", \"M\", \"Z\"}");
    }

    #[test]
    fn consulted_env_record_dedups() {
        let mut c = ConsultedEnv::new();
        c.record("CFLAGS");
        c.record("CFLAGS");
        c.record("CFLAGS");
        assert_eq!(c.keys.len(), 1);
    }

    #[test]
    fn consulted_env_to_lua_table_escapes_quotes() {
        let mut c = ConsultedEnv::new();
        c.record("KEY\"WITH\"QUOTES");
        assert!(c.to_lua_table().contains("\\\""));
    }

    // ─── expand_sigil_template tests ─────────────────────────────────────────

    fn empty_recipes() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn ctx_oneone_single(recipes: &BTreeSet<String>) -> ResolveCtx<'_> {
        ResolveCtx {
            mode: IterMode::OneToOne,
            outputs: OutputShape::Single,
            recipes_in_scope: recipes,
        }
    }

    fn ctx_oneshot_none(recipes: &BTreeSet<String>) -> ResolveCtx<'_> {
        ResolveCtx {
            mode: IterMode::OneShot,
            outputs: OutputShape::None,
            recipes_in_scope: recipes,
        }
    }

    #[test]
    fn no_placeholders_returns_quoted_literal() {
        let r = empty_recipes();
        let ctx = ctx_oneone_single(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("echo hello", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"echo hello\"");
        assert!(env.keys.is_empty());
    }

    #[test]
    fn shell_brace_idioms_survive_verbatim() {
        // {a,b,c}, ${HOME:-x}, awk '{print $1}' — none of these are $<...> so they pass through.
        let r = empty_recipes();
        let ctx = ctx_oneshot_none(&r);
        let mut env = ConsultedEnv::new();

        let result = expand_sigil_template("{a,b,c}", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"{a,b,c}\"");

        let result2 = expand_sigil_template("${HOME:-x}", &ctx, &mut env).unwrap();
        assert_eq!(result2, "\"${HOME:-x}\"");

        let result3 = expand_sigil_template("awk '{print $1}'", &ctx, &mut env).unwrap();
        assert_eq!(result3, "\"awk '{print $1}'\"");

        assert!(env.keys.is_empty(), "no env keys should be recorded for shell braces");
    }

    #[test]
    fn in_lowers_to_cook_in_in_one_to_one() {
        let r = empty_recipes();
        let ctx = ctx_oneone_single(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("$<in>", &ctx, &mut env).unwrap();
        assert_eq!(result, "_cook_in");
    }

    #[test]
    fn out_lowers_to_cook_out_in_single_output() {
        let r = empty_recipes();
        let ctx = ctx_oneone_single(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("$<out>", &ctx, &mut env).unwrap();
        assert_eq!(result, "_cook_out");
    }

    #[test]
    fn recipe_lowers_to_dep_output() {
        let mut r = BTreeSet::new();
        r.insert("libmath".to_string());
        let ctx = ctx_oneshot_none(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("gcc $<libmath>", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"gcc \" .. cook.dep_output(\"libmath\")");
    }

    #[test]
    fn env_var_lowers_to_require_env() {
        let r = empty_recipes();
        let ctx = ctx_oneshot_none(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("$<HOME>", &ctx, &mut env).unwrap();
        assert_eq!(result, "cook.require_env(\"HOME\")");
        assert!(env.keys.contains("HOME"), "HOME should be recorded");
    }

    #[test]
    fn env_prefix_strips_to_require_env() {
        let r = empty_recipes();
        let ctx = ctx_oneshot_none(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("$<env.HOME>", &ctx, &mut env).unwrap();
        assert_eq!(result, "cook.require_env(\"HOME\")");
        assert!(env.keys.contains("HOME"));
    }

    #[test]
    fn builtin_in_wrong_mode_returns_err() {
        let r = empty_recipes();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Single,
            recipes_in_scope: &r,
        };
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("$<in>", &ctx, &mut env);
        assert!(result.is_err(), "expected error for $<in> in many-to-one mode");
    }

    #[test]
    fn mixed_template_with_literal_and_placeholders() {
        let r = empty_recipes();
        let ctx = ctx_oneone_single(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("gcc -c $<in> -o $<out>", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out");
    }

    #[test]
    fn in_stem_expands_to_path_stem() {
        let r = empty_recipes();
        let ctx = ctx_oneone_single(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("build/$<in.stem>.o", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"build/\" .. path.stem(_cook_in) .. \".o\"");
    }
}

// ─── sigil_template_tests module ─────────────────────────────────────────────
#[cfg(test)]
mod sigil_template_tests {
    use super::*;

    fn empty_recipes() -> BTreeSet<String> { BTreeSet::new() }

    fn ctx_os_n0(r: &BTreeSet<String>) -> ResolveCtx<'_> {
        ResolveCtx { mode: IterMode::OneShot, outputs: OutputShape::None, recipes_in_scope: r }
    }

    #[test]
    fn empty_string() {
        let r = empty_recipes();
        let ctx = ctx_os_n0(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"\"");
    }

    #[test]
    fn all_in_many_to_one() {
        let r = empty_recipes();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Single,
            recipes_in_scope: &r,
        };
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("ar rcs $<out> $<all>", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"ar rcs \" .. _cook_out .. \" \" .. _cook_all");
    }

    #[test]
    fn out_n_in_multi_output() {
        let r = empty_recipes();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("cp $<out_1> $<out_2>", &ctx, &mut env).unwrap();
        assert_eq!(result, "\"cp \" .. _cook_outs[1] .. \" \" .. _cook_outs[2]");
    }

    #[test]
    fn recipe_with_accessor() {
        let mut r = BTreeSet::new();
        r.insert("lib".to_string());
        let ctx = ctx_os_n0(&r);
        let mut env = ConsultedEnv::new();
        let result = expand_sigil_template("$<lib>", &ctx, &mut env).unwrap();
        assert_eq!(result, "cook.dep_output(\"lib\")");
    }
}
