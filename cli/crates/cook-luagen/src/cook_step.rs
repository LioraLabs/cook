use cook_lang::ast::*;

use crate::template::{expand_output_pattern, expand_template_to_lua};

/// Modes for cook step code generation.
pub(crate) enum CookMode {
    /// Loop over inputs, producing one output per input.
    OneToOne,
    /// Single invocation combining all inputs.
    ManyToOne,
    /// No using clause -- just declare outputs, emit no code.
    DeclarationOnly,
}

pub(crate) fn cook_step_mode(step: &CookStep) -> CookMode {
    match &step.using_clause {
        None => CookMode::DeclarationOnly,
        Some(UsingClause::LuaBlock(_)) => CookMode::OneToOne,
        Some(UsingClause::Shell(cmd)) => {
            if cmd.contains("{in}") {
                CookMode::OneToOne
            } else {
                CookMode::ManyToOne
            }
        }
    }
}

pub(crate) fn generate_cook_step(
    out: &mut String,
    cook_step: &CookStep,
    _line: usize,
    index: usize,
    prev_cook_index: Option<usize>,
    ingredients: &[String],
) {
    let mode = cook_step_mode(cook_step);
    let input_source = if let Some(prev) = prev_cook_index {
        format!("_cook_outputs_{}", prev)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    match mode {
        CookMode::DeclarationOnly => {
            out.push_str(&format!(
                "    local _cook_outputs_{} = {{\"{}\"}}\n",
                index,
                crate::lua_string::escape_lua_string(&cook_step.output_pattern)
            ));
        }
        CookMode::OneToOne => {
            out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", index));
            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                input_source
            ));
            out.push_str("        local _cook_stem = path.stem(_cook_in)\n");
            out.push_str("        local _cook_name = path.name(_cook_in)\n");
            out.push_str("        local _cook_ext = path.ext(_cook_in)\n");
            out.push_str("        local _cook_dir = path.dir(_cook_in)\n");

            // Generate output expression
            let out_expr = expand_output_pattern(&cook_step.output_pattern);
            out.push_str(&format!("        local _cook_out = {}\n", out_expr));

            match &cook_step.using_clause {
                Some(UsingClause::Shell(cmd)) => {
                    let lua_expr = expand_template_to_lua(cmd);
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}}})\n",
                        lua_expr
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    out.push_str("        cook.add_unit({inputs = {_cook_in}, output = _cook_out, lua = function()\n");
                    out.push_str("            local input = _cook_in\n");
                    out.push_str("            local output = _cook_out\n");
                    // Expose all ingredient groups
                    for (i, _) in ingredients.iter().enumerate() {
                        out.push_str(&format!(
                            "            local input_{} = recipe.ingredients[{}]\n",
                            i + 1,
                            i + 1
                        ));
                    }
                    for code_line in code.lines() {
                        out.push_str(&format!("            {}\n", code_line));
                    }
                    out.push_str("        end})\n");
                }
                None => {
                    // unreachable in OneToOne
                }
            }

            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
            out.push_str("    end\n");
        }
        CookMode::ManyToOne => {
            out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", index));
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \")\n",
                input_source
            ));

            let out_expr = expand_output_pattern(&cook_step.output_pattern);
            out.push_str(&format!("    local _cook_out = {}\n", out_expr));

            if let Some(UsingClause::Shell(cmd)) = &cook_step.using_clause {
                let lua_expr = expand_template_to_lua(cmd);
                out.push_str(&format!(
                    "    cook.add_unit({{inputs = {}, output = _cook_out, command = {}}})\n",
                    input_source, lua_expr
                ));
            }

            out.push_str(&format!(
                "    table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
        }
    }
}
