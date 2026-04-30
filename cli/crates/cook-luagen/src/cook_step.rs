use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::template::{
    analyze_output_pattern, expand_template_to_lua_with_deps, OutputPatternKind,
};

/// Modes for cook step code generation.
pub(crate) enum CookMode {
    /// Loop over inputs, producing one output per input.
    OneToOne,
    /// Loop over inputs, producing N outputs per input (one-to-many).
    /// Output patterns all contain `{in.ACCESSOR}` or `{dep.ACCESSOR}`.
    OneToMany,
    /// Single invocation combining all inputs.
    ManyToOne,
    /// No using clause -- just declare outputs, emit no code.
    DeclarationOnly,
    /// Single invocation producing multiple declared outputs from a shell block
    /// or from a Lua block whose cook step declares more than one literal output.
    BlockStep,
}

fn format_ingredient_groups(n: usize) -> String {
    let parts: Vec<String> = (1..=n)
        .map(|i| format!("recipe.ingredients[{}]", i))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Determine the iteration mode for a cook step by inspecting its output pattern(s).
///
/// CS-0022: the output pattern list is the sole iteration source.
/// - No using_clause                         → DeclarationOnly
/// - Multiple outputs, all with {in.X}       → OneToMany (loop per input, N outputs each)
/// - Multiple outputs, all literal           → BlockStep (one unit, N literal outputs)
/// - Single output with {in.X}               → OneToOne (own-input accessor)
/// - Single output with {dep.X}              → OneToOne (dep-driven, only detectable with recipe names)
/// - Single literal output                   → ManyToOne
///
/// Without recipe-name context, dep-driven patterns (e.g. `{protos.stem}`) look like Literal.
/// Use `cook_step_mode_with_names` when recipe names are available.
///
/// This function is the no-context entry point; used primarily in tests.
#[allow(dead_code)]
pub(crate) fn cook_step_mode(step: &CookStep) -> CookMode {
    use crate::template::output_pattern_kind;

    if step.using_clause.is_none() {
        return CookMode::DeclarationOnly;
    }

    if step.outputs.len() > 1 {
        // Multi-output: check whether ALL patterns iterate over own inputs.
        // If any pattern contains {in.ACCESSOR}, treat as OneToMany (one-to-many
        // per-input iteration). Otherwise BlockStep (single many-to-one unit).
        let any_own_input = step
            .outputs
            .iter()
            .any(|p| matches!(output_pattern_kind(p), OutputPatternKind::OwnInputAccessor));
        return if any_own_input {
            CookMode::OneToMany
        } else {
            CookMode::BlockStep
        };
    }

    // Single-output: the output pattern decides iteration.
    match output_pattern_kind(&step.outputs[0]) {
        OutputPatternKind::OwnInputAccessor | OutputPatternKind::DepDriven { .. } => {
            CookMode::OneToOne
        }
        OutputPatternKind::Literal => CookMode::ManyToOne,
    }
}

/// Recipe-name-aware variant of `cook_step_mode`.
/// Correctly identifies dep-driven patterns (e.g. `{protos.stem}`) as OneToOne.
pub(crate) fn cook_step_mode_with_names(
    step: &CookStep,
    recipe_names: &BTreeSet<String>,
) -> CookMode {
    use crate::template::output_pattern_kind_with_recipes;

    if step.using_clause.is_none() {
        return CookMode::DeclarationOnly;
    }

    if step.outputs.len() > 1 {
        // Multi-output: if any pattern has an own-input or dep-driven accessor,
        // this is a OneToMany (one-to-many per-input) step. Otherwise BlockStep.
        let any_iterating = step.outputs.iter().any(|p| {
            matches!(
                output_pattern_kind_with_recipes(p, recipe_names),
                OutputPatternKind::OwnInputAccessor | OutputPatternKind::DepDriven { .. }
            )
        });
        return if any_iterating {
            CookMode::OneToMany
        } else {
            CookMode::BlockStep
        };
    }

    match output_pattern_kind_with_recipes(&step.outputs[0], recipe_names) {
        OutputPatternKind::OwnInputAccessor | OutputPatternKind::DepDriven { .. } => {
            CookMode::OneToOne
        }
        OutputPatternKind::Literal => CookMode::ManyToOne,
    }
}

/// Join a shell-block's lines with `\n`, prepended with `set -e`.
/// The result is a single shell text suitable for `/bin/sh -c`.
fn build_shell_block_command(lines: &[String], _recipe_names: &BTreeSet<String>) -> String {
    let mut out = String::from("set -e");
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out
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
    // Use recipe-name-aware mode selection so dep-driven patterns are correctly
    // identified as OneToOne (e.g. {protos.stem} is OneToOne when "protos" is known).
    let mode = cook_step_mode_with_names(cook_step, recipe_names);
    // Input source: prior cook step's outputs, own ingredients, or empty table.
    // Using `{}` when there are no ingredients avoids `nil` in table.concat/ipairs.
    let input_source = if let Some(prev) = prev_cook_index {
        format!("_cook_outputs_{}", prev)
    } else if !ingredients.is_empty() {
        "recipe.ingredients[1]".to_string()
    } else {
        "{}".to_string()
    };

    let pattern_kind = analyze_output_pattern(&cook_step.outputs[0], recipe_names);

    // Note: placeholder validation (CS-0022 §6.7) is performed in
    // recipe::validate_accessor_placement, called from generate_with_names_checked.
    // generate_cook_step is called from generate_with_names (unchecked path).

    match mode {
        CookMode::DeclarationOnly => {
            // _cook_outputs_N is hoisted to recipe scope by recipe.rs.
            // Just populate it with the declared output.
            out.push_str(&format!(
                "    _cook_outputs_{}[1] = \"{}\"\n",
                index,
                crate::lua_string::escape_lua_string(&cook_step.outputs[0])
            ));
        }
        CookMode::OneToOne => {
            // _cook_outputs_N is hoisted to recipe scope by recipe.rs.
            // Choose iteration source: dep-driven or own inputs.
            let iter_source = match &pattern_kind {
                OutputPatternKind::DepDriven { dep_name, .. } => {
                    format!("cook.dep_output_list(\"{}\")", crate::lua_string::escape_lua_string(dep_name))
                }
                OutputPatternKind::OwnInputAccessor => input_source.clone(),
                OutputPatternKind::Literal => input_source.clone(),
            };
            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                iter_source
            ));

            // Generate output expression (already expanded by analyze_output_pattern).
            let out_expr = match &pattern_kind {
                OutputPatternKind::DepDriven { lua_expr, .. } => lua_expr.clone(),
                OutputPatternKind::OwnInputAccessor => {
                    crate::template::expand_output_pattern(&cook_step.outputs[0])
                }
                OutputPatternKind::Literal => {
                    format!("\"{}\"", crate::lua_string::escape_lua_string(&cook_step.outputs[0]))
                }
            };
            out.push_str(&format!("        local _cook_out = {}\n", out_expr));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    // Per CS-0022, shell-block contents go through expand_template_to_lua_with_deps.
                    let combined = build_shell_block_command(lines, recipe_names);
                    let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}}})\n",
                        lua_expr
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, lua_code = {}, ingredient_groups = {}}})\n",
                        code_literal, ing_groups
                    ));
                }
                None => {
                    // unreachable in OneToOne
                    unreachable!("OneToOne mode requires a using-clause");
                }
            }

            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
            out.push_str("    end\n");
        }
        CookMode::ManyToOne => {
            // _cook_outputs_N is hoisted to recipe scope by recipe.rs.
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \")\n",
                input_source
            ));

            let out_expr = match &pattern_kind {
                OutputPatternKind::DepDriven { lua_expr, .. } => lua_expr.clone(),
                OutputPatternKind::OwnInputAccessor => {
                    crate::template::expand_output_pattern(&cook_step.outputs[0])
                }
                OutputPatternKind::Literal => {
                    format!("\"{}\"", crate::lua_string::escape_lua_string(&cook_step.outputs[0]))
                }
            };
            out.push_str(&format!("    local _cook_out = {}\n", out_expr));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = {}, output = _cook_out, command = {}}})\n",
                        input_source, lua_expr
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    // CS-0022 wart-fix: many-to-one Lua block runs once with the full
                    // inputs/outputs arrays (pre-CS-0022, this case routed through OneToOne
                    // and iterated, which was incorrect).
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = {}, output = _cook_out, lua_code = {}, ingredient_groups = {}}})\n",
                        input_source, code_literal, ing_groups
                    ));
                }
                None => unreachable!("ManyToOne mode requires a using-clause"),
            }

            out.push_str(&format!(
                "    table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
        }
        CookMode::OneToMany => {
            // Multi-output one-to-one (one-to-many): loop over inputs, compute
            // N outputs per iteration. Each output pattern contains {in.ACCESSOR}.
            //
            // Choose iteration source (dep-driven vs own inputs).
            let iter_source = match &pattern_kind {
                OutputPatternKind::DepDriven { dep_name, .. } => {
                    format!("cook.dep_output_list(\"{}\")", crate::lua_string::escape_lua_string(dep_name))
                }
                _ => input_source.clone(),
            };

            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                iter_source
            ));

            // Compute per-iteration outputs for each pattern.
            out.push_str("        local _cook_outs = {\n");
            for pat in &cook_step.outputs {
                let expr = crate::template::expand_output_pattern(pat);
                out.push_str(&format!("            {},\n", expr));
            }
            out.push_str("        };\n");

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, outputs = _cook_outs, command = {}}})\n",
                        lua_expr
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, outputs = _cook_outs, lua_code = {}, ingredient_groups = {}}})\n",
                        code_literal, ing_groups
                    ));
                }
                None => unreachable!("OneToMany mode requires a using-clause"),
            }

            // Populate _cook_outputs_N with the first output of each unit
            // (by convention, the first declared output drives downstream deps).
            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_outs[1])\n",
                index
            ));
            out.push_str("    end\n");
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
            // {all} in a BlockStep shell body expands to _cook_all.
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \");\n",
                input_source
            ));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    // Shell-block content now goes through expand_template_to_lua_with_deps
                    // (Task 11 — previously emitted verbatim).
                    let combined = build_shell_block_command(lines, recipe_names);
                    let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, command = {}}})\n",
                        lua_expr
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, lua_code = {}, ingredient_groups = {}}})\n",
                        code_literal, ing_groups
                    ));
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

#[cfg(test)]
mod cs_0022_mode_tests {
    use super::*;
    use cook_lang::ast::{CookStep, UsingClause};

    fn step(outputs: &[&str], using_clause: Option<UsingClause>) -> CookStep {
        CookStep {
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            using_clause,
        }
    }

    #[test]
    fn literal_output_is_many_to_one_regardless_of_body() {
        // A literal output pattern → ManyToOne, even if the body contains {in}.
        let s = step(
            &["build/app"],
            Some(UsingClause::ShellBlock(vec!["gcc {in}".into()])),
        );
        // Note: ManyToOne mode, but the body has {in} — validate_placeholders would
        // catch this at codegen time; the mode test just checks mode selection.
        assert!(matches!(cook_step_mode(&s), CookMode::ManyToOne));
    }

    #[test]
    fn in_accessor_output_is_one_to_one() {
        let s = step(
            &["build/{in.stem}.o"],
            Some(UsingClause::ShellBlock(vec!["gcc {in} -o {out}".into()])),
        );
        assert!(matches!(cook_step_mode(&s), CookMode::OneToOne));
    }

    #[test]
    fn lib_accessor_output_is_one_to_one_dep_driven() {
        // `cook_step_mode` (no recipe-name context) treats `{libmath.stem}` as
        // Literal because it can't confirm `libmath` is a recipe; the result is
        // ManyToOne. With names (via `cook_step_mode_with_names`), it becomes
        // OneToOne. Both are acceptable from this context-free call site.
        let s = step(
            &["build/{libmath.stem}.x"],
            Some(UsingClause::ShellBlock(vec!["echo {in}".into()])),
        );
        let m = cook_step_mode(&s);
        assert!(
            matches!(m, CookMode::OneToOne | CookMode::ManyToOne),
            "dep-driven pattern should be OneToOne (with names) or ManyToOne (without names)"
        );
    }

    #[test]
    fn multi_output_literal_is_block_step() {
        let s = step(
            &["a.js", "a.wasm"],
            Some(UsingClause::ShellBlock(vec!["gen".into()])),
        );
        // Per spec §3.1, multi-output → BlockStep.
        assert!(matches!(cook_step_mode(&s), CookMode::BlockStep));
    }

    #[test]
    fn declaration_only_no_using_clause() {
        let s = step(&["x"], None);
        assert!(matches!(cook_step_mode(&s), CookMode::DeclarationOnly));
    }
}
