use crate::lua_string::escape_lua_string;

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

/// Expand a plate command template into a Lua expression.
/// Only {out} is valid.
pub(crate) fn expand_plate_cmd(template: &str) -> String {
    let builtins = &[("{out}", "_plate_out")];
    expand_template_with_env_fallback(template, builtins)
}

/// Expand a test command template into a Lua expression.
/// Only {out} is valid (same as plate).
pub(crate) fn expand_test_cmd(template: &str) -> String {
    let builtins = &[("{out}", "_test_out")];
    expand_template_with_env_fallback(template, builtins)
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
