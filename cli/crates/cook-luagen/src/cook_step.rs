use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::template::{analyze_output_pattern, expand_template_to_lua_with_deps, OutputPatternKind};

/// Modes for cook step code generation.
pub(crate) enum CookMode {
    /// Loop over inputs, producing one output per input.
    OneToOne,
    /// Single invocation combining all inputs.
    ManyToOne,
    /// No using clause -- just declare outputs, emit no code.
    DeclarationOnly,
    /// Single invocation producing multiple declared outputs from a shell block
    /// or from a Lua block whose cook step declares more than one output.
    BlockStep,
}

pub(crate) fn cook_step_mode(step: &CookStep) -> CookMode {
    match &step.using_clause {
        None => CookMode::DeclarationOnly,
        Some(UsingClause::ShellBlock(_)) => CookMode::BlockStep,
        Some(UsingClause::LuaBlock(_)) if step.outputs.len() > 1 => CookMode::BlockStep,
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
    recipe_names: &BTreeSet<String>,
) {
    let mode = cook_step_mode(cook_step);
    let input_source = if let Some(prev) = prev_cook_index {
        format!("_cook_outputs_{}", prev)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    let pattern_kind = analyze_output_pattern(&cook_step.outputs[0], recipe_names);

    match mode {
        CookMode::DeclarationOnly => {
            // _cook_outputs_N is hoisted to recipe scope by recipe.rs
            // Just populate it with the declared output
            out.push_str(&format!(
                "    _cook_outputs_{}[1] = \"{}\"\n",
                index,
                crate::lua_string::escape_lua_string(&cook_step.outputs[0])
            ));
        }
        CookMode::OneToOne => {
            // _cook_outputs_N is hoisted to recipe scope by recipe.rs
            // Choose iteration source: dep-driven or own inputs.
            let iter_source = match &pattern_kind {
                OutputPatternKind::DepDriven { dep_name, .. } => {
                    format!("cook.dep_output_list(\"{}\")", crate::lua_string::escape_lua_string(dep_name))
                }
                OutputPatternKind::OwnInputs(_) => input_source.clone(),
            };
            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                iter_source
            ));
            out.push_str("        local _cook_stem = path.stem(_cook_in)\n");
            out.push_str("        local _cook_name = path.name(_cook_in)\n");
            out.push_str("        local _cook_ext = path.ext(_cook_in)\n");
            out.push_str("        local _cook_dir = path.dir(_cook_in)\n");

            // Generate output expression (already expanded by analyze_output_pattern)
            let out_expr = match &pattern_kind {
                OutputPatternKind::DepDriven { lua_expr, .. } => lua_expr.clone(),
                OutputPatternKind::OwnInputs(lua_expr) => lua_expr.clone(),
            };
            out.push_str(&format!("        local _cook_out = {}\n", out_expr));

            match &cook_step.using_clause {
                Some(UsingClause::Shell(cmd)) => {
                    let lua_expr = expand_template_to_lua_with_deps(cmd, recipe_names);
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
                Some(UsingClause::ShellBlock(_)) => {
                    // ShellBlock routes to CookMode::BlockStep; unreachable here.
                    unreachable!("ShellBlock should be handled by BlockStep arm");
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
            // _cook_outputs_N is hoisted to recipe scope by recipe.rs
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \")\n",
                input_source
            ));

            let out_expr = match &pattern_kind {
                OutputPatternKind::DepDriven { lua_expr, .. } => lua_expr.clone(),
                OutputPatternKind::OwnInputs(lua_expr) => lua_expr.clone(),
            };
            out.push_str(&format!("    local _cook_out = {}\n", out_expr));

            if let Some(UsingClause::Shell(cmd)) = &cook_step.using_clause {
                let lua_expr = expand_template_to_lua_with_deps(cmd, recipe_names);
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
        CookMode::BlockStep => {
            // Build the Lua table of declared outputs.
            let mut outs_lua = String::from("{");
            for (i, out_name) in cook_step.outputs.iter().enumerate() {
                if i > 0 {
                    outs_lua.push_str(", ");
                }
                outs_lua.push('"');
                outs_lua.push_str(&crate::lua_string::escape_lua_string(out_name));
                outs_lua.push('"');
            }
            outs_lua.push('}');

            out.push_str(&format!("    local _cook_outs = {};\n", outs_lua));
            out.push_str(&format!("    local _cook_ins = {};\n", input_source));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    // Join lines with \n, prepend `set -e` for fail-fast.
                    // The combined command is executed by the Shell worker via /bin/sh -c.
                    let mut combined = String::from("set -e");
                    for shell_line in lines {
                        combined.push('\n');
                        combined.push_str(shell_line);
                    }
                    let escaped = crate::lua_string::escape_lua_string(&combined);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, command = \"{}\"}})\n",
                        escaped
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    // Known runtime gap: cook-register's unit_api does not currently read the
                    // `lua` key on cook.add_unit, so this function body is dropped at runtime.
                    // This matches the pre-existing OneToOne LuaBlock behavior; tracked as a
                    // follow-up and intentionally left unchanged in this chunk.
                    out.push_str("    cook.add_unit({inputs = _cook_ins, outputs = _cook_outs, lua = function()\n");
                    out.push_str("        local inputs = _cook_ins\n");
                    out.push_str("        local outputs = _cook_outs\n");
                    for (i, _) in ingredients.iter().enumerate() {
                        out.push_str(&format!(
                            "        local input_{} = recipe.ingredients[{}]\n",
                            i + 1,
                            i + 1
                        ));
                    }
                    for code_line in code.lines() {
                        out.push_str(&format!("        {}\n", code_line));
                    }
                    out.push_str("    end})\n");
                }
                _ => unreachable!("BlockStep mode requires ShellBlock or LuaBlock using-clause"),
            }

            // Populate _cook_outputs_N with the declared outputs in order.
            for out_idx in 0..cook_step.outputs.len() {
                out.push_str(&format!(
                    "    table.insert(_cook_outputs_{}, _cook_outs[{}])\n",
                    index,
                    out_idx + 1
                ));
            }
        }
    }
}
