use std::collections::BTreeSet;

use crate::cook_step::CookMode;
use crate::lua_string::escape_lua_string;

/// Known accessor suffixes for dep-driven iteration patterns.
const DEP_ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

/// Result of analyzing an output pattern for dep-driven or own-input iteration.
pub(crate) enum OutputPatternKind {
    /// Pattern is fully literal — many-to-one (no iteration source in output).
    Literal,
    /// Pattern contains `{in.ACCESSOR}` — one-to-one over own inputs.
    OwnInputAccessor,
    /// Pattern contains `{recipe.ACCESSOR}` — one-to-one over that dep's outputs.
    #[allow(dead_code)]
    DepDriven { dep_name: String, accessor: String, lua_expr: String },
}

/// Determine the iteration kind of an output pattern WITHOUT recipe-name knowledge.
/// Checks for `{in.ACCESSOR}` first, then does a best-effort dep check with empty names.
/// Used primarily in `cook_step_mode` (test-facing entry point).
#[allow(dead_code)]
pub(crate) fn output_pattern_kind(pattern: &str) -> OutputPatternKind {
    if pattern.contains("{in.") {
        return OutputPatternKind::OwnInputAccessor;
    }
    if let Some((dep, accessor)) = first_dep_accessor(pattern, &Default::default()) {
        return OutputPatternKind::DepDriven {
            dep_name: dep,
            accessor,
            lua_expr: String::new(), // computed at call-site when needed
        };
    }
    OutputPatternKind::Literal
}

/// Determine the iteration kind of an output pattern WITH full recipe-name knowledge.
pub(crate) fn output_pattern_kind_with_recipes(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    if pattern.contains("{in.") {
        return OutputPatternKind::OwnInputAccessor;
    }
    if let Some((dep, accessor)) = first_dep_accessor(pattern, recipe_names) {
        let lua_expr = expand_output_pattern_with_recipe(&dep, &accessor, pattern, recipe_names);
        return OutputPatternKind::DepDriven { dep_name: dep, accessor, lua_expr };
    }
    OutputPatternKind::Literal
}

/// Walk a pattern's `{TOKEN.SUFFIX}` placeholders and return the first whose
/// TOKEN is in `recipe_names` and SUFFIX is a known path accessor.
fn first_dep_accessor(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> Option<(String, String)> {
    let tokens = crate::dep_ref::extract_brace_tokens(pattern);
    for token in &tokens {
        if let Some(dot_pos) = token.rfind('.') {
            let prefix = &token[..dot_pos];
            let suffix = &token[dot_pos + 1..];
            if DEP_ACCESSORS.contains(&suffix) && recipe_names.contains(prefix) {
                return Some((prefix.to_string(), suffix.to_string()));
            }
        }
    }
    None
}

/// Analyze an output pattern and determine whether iteration should be driven
/// by a dependency's terminal outputs or by the recipe's own inputs.
///
/// Legacy entry-point kept for callers that still use OwnInputs-style patterns
/// (dep-driven with {recipe.accessor} in the output pattern).
pub(crate) fn analyze_output_pattern(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    output_pattern_kind_with_recipes(pattern, recipe_names)
}

/// Expand an output pattern like "build/{in.stem}.o" into a Lua expression.
///
/// Supported placeholders in output patterns (CS-0022):
///   {in.stem} / {in.name} / {in.ext} / {in.dir}  → path.ACCESSOR(_cook_in)
///   {in}                                           → _cook_in
///   {dep.stem} / {dep.name} / ...                 → path.ACCESSOR(_cook_in)  (dep-driven, normalized)
///   everything else                               → cook.env["TOKEN"]
pub(crate) fn expand_output_pattern(pattern: &str) -> String {
    // For OwnInputAccessor patterns and normalised dep patterns, the {in.X} and
    // bare {in} placeholders map to the loop variable.  Unknown tokens fall
    // back to cook.env.
    expand_output_pattern_inner(pattern)
}

fn expand_output_pattern_inner(pattern: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = pattern;

    while !remaining.is_empty() {
        let brace_pos = remaining.find('{');
        match brace_pos {
            None => {
                parts.push(format!("\"{}\"", escape_lua_string(remaining)));
                break;
            }
            Some(brace_start) => {
                if brace_start > 0 {
                    parts.push(format!("\"{}\"", escape_lua_string(&remaining[..brace_start])));
                }
                let after_brace = &remaining[brace_start..];
                if let Some(close) = after_brace.find('}') {
                    let inner = &after_brace[1..close];
                    let lua = expand_output_token(inner);
                    parts.push(lua);
                    remaining = &remaining[brace_start + close + 1..];
                } else {
                    parts.push(format!("\"{}\"", escape_lua_string(&remaining[brace_start..])));
                    break;
                }
            }
        }
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}

/// Expand a single brace-token inside an output pattern.
fn expand_output_token(inner: &str) -> String {
    // {in} → _cook_in
    if inner == "in" {
        return "_cook_in".to_string();
    }
    // {in.ACCESSOR} → path.ACCESSOR(_cook_in)
    if let Some(acc) = inner.strip_prefix("in.") {
        if DEP_ACCESSORS.contains(&acc) {
            return format!("path.{}(_cook_in)", acc);
        }
    }
    // {out} → _cook_out (sometimes used in output patterns for chaining — rare)
    if inner == "out" {
        return "_cook_out".to_string();
    }
    // {out.ACCESSOR} → path.ACCESSOR(_cook_out)
    if let Some(acc) = inner.strip_prefix("out.") {
        if DEP_ACCESSORS.contains(&acc) {
            return format!("path.{}(_cook_out)", acc);
        }
    }
    // Bare old-style {stem}/{name}/{ext}/{dir} — treat as {in.ACCESSOR} for backward compat
    // in output pattern expansion only (the dep-driven normalization path strips the dep prefix
    // and passes the bare accessor through here).
    if DEP_ACCESSORS.contains(&inner) {
        // This is the normalized dep-driven path (dep.accessor → accessor).
        return format!("path.{}(_cook_in)", inner);
    }
    // Anything else: cook.env fallback
    format!("cook.env[\"{}\"]", escape_lua_string(inner))
}

/// Build the Lua expression for a dep-driven output pattern.
/// Normalizes `{dep.accessor}` → `{accessor}` then expands.
fn expand_output_pattern_with_recipe(
    dep_name: &str,
    _accessor: &str,
    pattern: &str,
    _recipe_names: &BTreeSet<String>,
) -> String {
    // Normalise: replace every `{dep.X}` with `{X}` so expand_output_pattern_inner handles it.
    // We look for all `{dep.ACCESSOR}` tokens and replace them.
    let mut normalized = pattern.to_string();
    for acc in DEP_ACCESSORS {
        normalized = normalized.replace(
            &format!("{{{}.{}}}", dep_name, acc),
            &format!("{{{}}}", acc),
        );
    }
    expand_output_pattern_inner(&normalized)
}

/// Expand a shell command template into a Lua expression (no dep-ref awareness).
/// Placeholders: {in}, {out}, {in.X}, {out.X}, {all}
/// Used in tests to verify individual placeholder expansion.
#[allow(dead_code)]
pub(crate) fn expand_template_to_lua(template: &str) -> String {
    expand_with_deps_fallback(template, &BTreeSet::new())
}

/// Expand a shell command template, checking recipe names before falling back to cook.env.
pub(crate) fn expand_template_to_lua_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    expand_with_deps_fallback(template, recipe_names)
}

/// Expand a plate command template, checking recipe names before falling back to cook.env.
pub(crate) fn expand_plate_cmd_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    // Plate commands only use {out} → _plate_out; everything else goes through
    // the standard dep/env fallback.
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        let brace_pos = remaining.find('{');
        match brace_pos {
            None => {
                parts.push(format!("\"{}\"", escape_lua_string(remaining)));
                break;
            }
            Some(brace_start) => {
                if brace_start > 0 {
                    parts.push(format!("\"{}\"", escape_lua_string(&remaining[..brace_start])));
                }
                let after_brace = &remaining[brace_start..];
                if let Some(close) = after_brace.find('}') {
                    let inner = &after_brace[1..close];
                    let lua = if inner == "out" {
                        "_plate_out".to_string()
                    } else if inner == "out.stem" || inner.starts_with("out.") {
                        if let Some(acc) = inner.strip_prefix("out.") {
                            if DEP_ACCESSORS.contains(&acc) {
                                format!("path.{}(_plate_out)", acc)
                            } else {
                                format!("cook.env[\"{}\"]", escape_lua_string(inner))
                            }
                        } else {
                            format!("cook.env[\"{}\"]", escape_lua_string(inner))
                        }
                    } else if recipe_names.contains(inner) {
                        format!("cook.dep_output(\"{}\")", escape_lua_string(inner))
                    } else {
                        format!("cook.env[\"{}\"]", escape_lua_string(inner))
                    };
                    parts.push(lua);
                    remaining = &remaining[brace_start + close + 1..];
                } else {
                    parts.push(format!("\"{}\"", escape_lua_string(&remaining[brace_start..])));
                    break;
                }
            }
        }
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}

/// Expand a test command template, checking recipe names before falling back to cook.env.
pub(crate) fn expand_test_cmd_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        let brace_pos = remaining.find('{');
        match brace_pos {
            None => {
                parts.push(format!("\"{}\"", escape_lua_string(remaining)));
                break;
            }
            Some(brace_start) => {
                if brace_start > 0 {
                    parts.push(format!("\"{}\"", escape_lua_string(&remaining[..brace_start])));
                }
                let after_brace = &remaining[brace_start..];
                if let Some(close) = after_brace.find('}') {
                    let inner = &after_brace[1..close];
                    let lua = if inner == "out" {
                        "_test_out".to_string()
                    } else if let Some(acc) = inner.strip_prefix("out.") {
                        if DEP_ACCESSORS.contains(&acc) {
                            format!("path.{}(_test_out)", acc)
                        } else {
                            format!("cook.env[\"{}\"]", escape_lua_string(inner))
                        }
                    } else if recipe_names.contains(inner) {
                        format!("cook.dep_output(\"{}\")", escape_lua_string(inner))
                    } else {
                        format!("cook.env[\"{}\"]", escape_lua_string(inner))
                    };
                    parts.push(lua);
                    remaining = &remaining[brace_start + close + 1..];
                } else {
                    parts.push(format!("\"{}\"", escape_lua_string(&remaining[brace_start..])));
                    break;
                }
            }
        }
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}

/// The core expansion engine for shell-text bodies (using { ... }, plate, test, bare shell).
///
/// Placeholder table (CS-0022 §6.7):
///
/// | Token          | Lua expression              |
/// |----------------|-----------------------------|
/// | `{in}`         | `_cook_in`                  |
/// | `{in.ACCESSOR}`| `path.ACCESSOR(_cook_in)`   |
/// | `{out}`        | `_cook_out`                 |
/// | `{out.ACCESSOR}`| `path.ACCESSOR(_cook_out)` |
/// | `{out_N}`      | `_cook_outs[N]` (1-indexed) |
/// | `{out_N.ACCESSOR}`| `path.ACCESSOR(_cook_outs[N])` |
/// | `{all}`        | `_cook_all`                 |
/// | `{NAME}`       | `cook.dep_output("NAME")` if NAME is a recipe, else `cook.env["NAME"]` |
///
/// Bare `{stem}`, `{name}`, `{ext}`, `{dir}` fall through to cook.env (no special treatment).
fn expand_with_deps_fallback(template: &str, recipe_names: &BTreeSet<String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        let brace_pos = remaining.find('{');

        match brace_pos {
            None => {
                parts.push(format!("\"{}\"", escape_lua_string(remaining)));
                break;
            }
            Some(brace_start) => {
                if brace_start > 0 {
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[..brace_start])
                    ));
                }

                let after_brace = &remaining[brace_start..];
                if let Some(close) = after_brace.find('}') {
                    let inner = &after_brace[1..close];
                    let lua = expand_body_token(inner, recipe_names);
                    parts.push(lua);
                    remaining = &remaining[brace_start + close + 1..];
                } else {
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[brace_start..])
                    ));
                    break;
                }
            }
        }
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}

/// Expand a single brace-token inside a shell-text body.
fn expand_body_token(inner: &str, recipe_names: &BTreeSet<String>) -> String {
    // {in} → _cook_in
    if inner == "in" {
        return "_cook_in".to_string();
    }
    // {in.ACCESSOR} → path.ACCESSOR(_cook_in)
    if let Some(acc) = inner.strip_prefix("in.") {
        if DEP_ACCESSORS.contains(&acc) {
            return format!("path.{}(_cook_in)", acc);
        }
    }
    // {out} → _cook_out
    if inner == "out" {
        return "_cook_out".to_string();
    }
    // {out.ACCESSOR} → path.ACCESSOR(_cook_out)
    if let Some(acc) = inner.strip_prefix("out.") {
        if DEP_ACCESSORS.contains(&acc) {
            return format!("path.{}(_cook_out)", acc);
        }
    }
    // {out_N} → _cook_outs[N]  (N ≥ 1, Lua 1-indexed)
    if let Some(n_str) = inner.strip_prefix("out_") {
        if !n_str.contains('.') {
            if let Ok(n) = n_str.parse::<usize>() {
                if n >= 1 {
                    return format!("_cook_outs[{}]", n);
                }
            }
        }
    }
    // {out_N.ACCESSOR} → path.ACCESSOR(_cook_outs[N])
    if inner.starts_with("out_") {
        if let Some(dot) = inner.find('.') {
            let n_part = &inner[4..dot]; // "out_" is 4 chars
            let acc = &inner[dot + 1..];
            if let Ok(n) = n_part.parse::<usize>() {
                if n >= 1 && DEP_ACCESSORS.contains(&acc) {
                    return format!("path.{}(_cook_outs[{}])", acc, n);
                }
            }
        }
    }
    // {all} → _cook_all
    if inner == "all" {
        return "_cook_all".to_string();
    }
    // {NAME} where NAME is a recipe name → cook.dep_output("NAME")
    if recipe_names.contains(inner) {
        return format!("cook.dep_output(\"{}\")", escape_lua_string(inner));
    }
    // Fallback: cook.env["TOKEN"]
    format!("cook.env[\"{}\"]", escape_lua_string(inner))
}

/// Iterate over all `{...}` placeholder tokens in a body text, yielding the
/// inner content (without braces).
pub(crate) fn iter_placeholders(body_text: &str) -> impl Iterator<Item = &str> {
    BodyPlaceholders { remaining: body_text }
}

struct BodyPlaceholders<'a> {
    remaining: &'a str,
}

impl<'a> Iterator for BodyPlaceholders<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let open = self.remaining.find('{')?;
            let after = &self.remaining[open + 1..];
            if let Some(close) = after.find('}') {
                let inner = &after[..close];
                self.remaining = &after[close + 1..];
                if !inner.is_empty() {
                    return Some(inner);
                }
                // empty braces — skip
            } else {
                self.remaining = "";
                return None;
            }
        }
    }
}

/// Validation context for `validate_placeholders`.
pub(crate) struct PlaceholderValidationContext<'a> {
    pub mode: &'a CookMode,
    pub declared_output_count: usize,
    pub recipe_names: &'a BTreeSet<String>,
}

/// Validate all `{...}` placeholders in `body_text` against the given context.
/// Returns `Err(diagnostic)` on the first violation.
pub(crate) fn validate_placeholders(
    body_text: &str,
    ctx: &PlaceholderValidationContext,
) -> Result<(), String> {
    for inner in iter_placeholders(body_text) {
        if let Some(dot) = inner.find('.') {
            let prefix = &inner[..dot];
            let suffix = &inner[dot + 1..];

            match prefix {
                "in" => {
                    if !is_iterating(ctx.mode) {
                        return Err(format!(
                            "CS-0022: {{in.{suffix}}} is invalid in many-to-one mode"
                        ));
                    }
                    if !is_path_accessor(suffix) {
                        return Err(format!(
                            "CS-0022: unknown accessor `{suffix}` (expected stem|name|ext|dir)"
                        ));
                    }
                }
                "out" => {
                    if ctx.declared_output_count != 1 {
                        return Err(format!(
                            "CS-0022: {{out.{suffix}}} requires single-output step (use {{out_N.{suffix}}})"
                        ));
                    }
                    if !is_path_accessor(suffix) {
                        return Err(format!(
                            "CS-0022: unknown accessor `{suffix}`"
                        ));
                    }
                }
                p if p.starts_with("out_") => {
                    let n: usize = p["out_".len()..]
                        .parse()
                        .map_err(|_| format!("CS-0022: invalid {{out_N}} index in `{p}`"))?;
                    if ctx.declared_output_count == 1 {
                        return Err(format!(
                            "CS-0022: {{out_{n}.{suffix}}} requires multi-output step (use {{out.{suffix}}})"
                        ));
                    }
                    if n < 1 || n > ctx.declared_output_count {
                        return Err(format!(
                            "CS-0022: {{out_{n}.{suffix}}} out of range (step has {} outputs)",
                            ctx.declared_output_count
                        ));
                    }
                    if !is_path_accessor(suffix) {
                        return Err(format!("CS-0022: unknown accessor `{suffix}`"));
                    }
                }
                lib if ctx.recipe_names.contains(lib) => {
                    return Err(format!(
                        "CS-0022: {{{lib}.{suffix}}} is rejected inside using-clause body; \
                         use {{in.{suffix}}} if `{lib}` is the driver, or reach for Lua otherwise"
                    ));
                }
                _ => { /* env var or other dotted form — fine */ }
            }
        } else {
            // Bare token
            match inner {
                "in" => {
                    if !is_iterating(ctx.mode) {
                        return Err(
                            "CS-0022: {in} is invalid in many-to-one mode".to_string()
                        );
                    }
                }
                "out" => {
                    if ctx.declared_output_count != 1 {
                        return Err(
                            "CS-0022: {out} requires single-output step (use {out_N} for multi-output)"
                                .to_string()
                        );
                    }
                }
                t if t.starts_with("out_") && !t.contains('.') => {
                    let n: usize = t["out_".len()..]
                        .parse()
                        .map_err(|_| format!("CS-0022: invalid {{out_N}} index in `{t}`"))?;
                    if ctx.declared_output_count == 1 {
                        return Err(format!(
                            "CS-0022: {{out_{n}}} requires multi-output step (use {{out}})"
                        ));
                    }
                    if n < 1 || n > ctx.declared_output_count {
                        return Err(format!(
                            "CS-0022: {{out_{n}}} out of range (step has {} outputs)",
                            ctx.declared_output_count
                        ));
                    }
                }
                "all" => {
                    if is_iterating(ctx.mode) {
                        return Err(
                            "CS-0022: {all} is invalid in one-to-one mode (use {in})".to_string()
                        );
                    }
                }
                "stem" | "name" | "ext" | "dir" => {
                    return Err(format!(
                        "CS-0022: bare {{{inner}}} was removed; use {{in.{inner}}} \
                         (or {{out.{inner}}} / {{out_N.{inner}}})"
                    ));
                }
                _ => { /* recipe name, env var, or other — fine at this layer */ }
            }
        }
    }
    Ok(())
}

fn is_iterating(m: &CookMode) -> bool {
    matches!(m, CookMode::OneToOne | CookMode::OneToMany)
}

fn is_path_accessor(s: &str) -> bool {
    matches!(s, "stem" | "name" | "ext" | "dir")
}
