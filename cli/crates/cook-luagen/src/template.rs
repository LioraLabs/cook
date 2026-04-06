use std::collections::BTreeSet;

use crate::lua_string::escape_lua_string;

/// Known accessor suffixes for dep-driven iteration patterns.
const DEP_ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

/// Result of analyzing an output pattern for dep-driven iteration.
pub(crate) enum OutputPatternKind {
    /// Normal pattern — iteration driven by recipe's own inputs.
    OwnInputs(String),
    /// Dep-driven — iteration driven by a dependency's terminal outputs.
    DepDriven { dep_name: String, lua_expr: String },
}

/// Analyze an output pattern and determine whether iteration should be driven
/// by a dependency's terminal outputs or by the recipe's own inputs.
///
/// If the pattern contains `{recipe.accessor}` where `recipe` is a known recipe
/// name and `accessor` is one of stem/name/ext/dir, returns `DepDriven` with
/// the dep name and the normalized Lua expression (with `{dep.accessor}` replaced
/// by `{accessor}` so `expand_output_pattern` can expand it normally).
pub(crate) fn analyze_output_pattern(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    let tokens = crate::dep_ref::extract_brace_tokens(pattern);

    for token in &tokens {
        if let Some(dot_pos) = token.rfind('.') {
            let prefix = &token[..dot_pos];
            let suffix = &token[dot_pos + 1..];
            if DEP_ACCESSORS.contains(&suffix) && recipe_names.contains(prefix) {
                // Found a dep-driven accessor. Normalize: replace {prefix.suffix} with {suffix}.
                let normalized = pattern.replace(
                    &format!("{{{}.{}}}", prefix, suffix),
                    &format!("{{{}}}", suffix),
                );
                let lua_expr = expand_output_pattern(&normalized);
                return OutputPatternKind::DepDriven {
                    dep_name: prefix.to_string(),
                    lua_expr,
                };
            }
        }
    }

    // No dep-driven accessor found — normal own-inputs expansion.
    OutputPatternKind::OwnInputs(expand_output_pattern(pattern))
}

/// Expand an output pattern like "build/{stem}.o" into a Lua expression.
/// Placeholders: {stem}, {name}, {ext}, {dir}, {in}
pub(crate) fn expand_output_pattern(pattern: &str) -> String {
    let builtins = &[
        ("{stem}", "_cook_stem"),
        ("{name}", "_cook_name"),
        ("{ext}", "_cook_ext"),
        ("{dir}", "_cook_dir"),
        ("{in}", "_cook_in"),
    ];
    expand_template_with_env_fallback(pattern, builtins)
}

/// Expand a shell command template into a Lua expression.
/// Placeholders: {in}, {out}, {stem}, {name}, {ext}, {dir}, {all}
pub(crate) fn expand_template_to_lua(template: &str) -> String {
    let builtins = &[
        ("{in}", "_cook_in"),
        ("{out}", "_cook_out"),
        ("{stem}", "_cook_stem"),
        ("{name}", "_cook_name"),
        ("{ext}", "_cook_ext"),
        ("{dir}", "_cook_dir"),
        ("{all}", "_cook_all"),
    ];
    expand_template_with_env_fallback(template, builtins)
}

/// Expand a shell command template, checking recipe names before falling back to cook.env.
pub(crate) fn expand_template_to_lua_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    let builtins = &[
        ("{in}", "_cook_in"),
        ("{out}", "_cook_out"),
        ("{stem}", "_cook_stem"),
        ("{name}", "_cook_name"),
        ("{ext}", "_cook_ext"),
        ("{dir}", "_cook_dir"),
        ("{all}", "_cook_all"),
    ];
    expand_with_deps_fallback(template, builtins, recipe_names)
}

/// Expand a plate command template, checking recipe names before falling back to cook.env.
pub(crate) fn expand_plate_cmd_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    let builtins = &[("{out}", "_plate_out")];
    expand_with_deps_fallback(template, builtins, recipe_names)
}

/// Expand a test command template, checking recipe names before falling back to cook.env.
pub(crate) fn expand_test_cmd_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    let builtins = &[("{out}", "_test_out")];
    expand_with_deps_fallback(template, builtins, recipe_names)
}

/// Two-pass template expansion with recipe-name awareness:
/// 1. Expand built-in placeholders ({in}, {out}, etc.) to Lua variable names
/// 2. For unknown {FOO}: emit cook.dep_output("FOO") if FOO is a recipe name, else cook.env["FOO"]
fn expand_with_deps_fallback(
    template: &str,
    builtins: &[(&str, &str)],
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
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[..brace_start])
                    ));
                }

                let after_brace = &remaining[brace_start..];
                if let Some(close) = after_brace.find('}') {
                    let placeholder = &after_brace[..close + 1];
                    let inner = &after_brace[1..close];

                    if let Some(&(_, var_name)) = builtins.iter().find(|&&(p, _)| p == placeholder)
                    {
                        parts.push(var_name.to_string());
                    } else if recipe_names.contains(inner) {
                        parts.push(format!("cook.dep_output(\"{}\")", escape_lua_string(inner)));
                    } else {
                        parts.push(format!("cook.env[\"{}\"]", escape_lua_string(inner)));
                    }

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

/// Two-pass template expansion:
/// 1. Expand built-in placeholders ({in}, {out}, etc.) to Lua variable names
/// 2. Expand remaining {VAR} tokens to cook.env.VAR lookups
fn expand_template_with_env_fallback(template: &str, builtins: &[(&str, &str)]) -> String {
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
                    let placeholder = &after_brace[..close + 1];
                    let inner = &after_brace[1..close];

                    if let Some(&(_, var_name)) = builtins.iter().find(|&&(p, _)| p == placeholder)
                    {
                        parts.push(var_name.to_string());
                    } else {
                        parts.push(format!("cook.env[\"{}\"]", escape_lua_string(inner)));
                    }

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
