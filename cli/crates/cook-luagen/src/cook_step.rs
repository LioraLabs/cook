use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::resolver::{IterMode, OutputShape};
use crate::template::{
    analyze_output_pattern, expand_command_template, expand_output_pattern, ConsultedEnv,
    OutputPatternKind,
};

/// Modes for cook step code generation.
pub(crate) enum CookMode {
    /// Loop over inputs, producing one output per input.
    OneToOne,
    /// Loop over inputs, producing N outputs per input (one-to-many).
    /// Output patterns all contain `$<in.ACCESSOR>` or `$<dep.ACCESSOR>`.
    OneToMany,
    /// Single invocation combining all inputs.
    ManyToOne,
    /// No using clause -- just declare outputs, emit no code.
    DeclarationOnly,
    /// Single invocation producing multiple declared outputs from a shell block
    /// or from a Lua block whose cook step declares more than one literal output.
    BlockStep,
    /// One-to-one over own ingredients with a per-ingredient Lua expression
    /// resolving the output path. Standard §8.4.2 (CS-0089): the parenthesised
    /// `cook (EXPR) using ...` form. Parser guarantees exactly one output and
    /// a using-clause.
    LuaExprOneToOne,
}

/// Render a set of probe keys as a Lua array literal: `{"key1", "key2"}`.
/// Returns `"{}"` for an empty set.
fn probe_keys_to_lua_table(keys: &BTreeSet<String>) -> String {
    if keys.is_empty() {
        return "{}".to_string();
    }
    let parts: Vec<String> = keys
        .iter()
        .map(|k| format!("\"{}\"", crate::lua_string::escape_lua_string(k)))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn format_ingredient_groups(n: usize) -> String {
    let parts: Vec<String> = (1..=n)
        .map(|i| format!("recipe.ingredients[{}]", i))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Determine the iteration mode for a cook step by inspecting its output pattern(s).
///
/// CS-0033: output patterns use `$<in.ACCESSOR>` for own-input iteration.
/// - No using_clause                             → DeclarationOnly
/// - Multiple outputs, any iterating              → OneToMany
/// - Multiple outputs, all literal                → BlockStep
/// - Single output with `$<in.X>`                 → OneToOne (own-input accessor)
/// - Single output with `$<dep.X>`                → OneToOne (dep-driven; needs recipe names)
/// - Single literal output                        → ManyToOne
///
/// Correctly identifies dep-driven patterns (e.g. `$<protos.stem>`) as OneToOne.
pub(crate) fn cook_step_mode_with_names(
    step: &CookStep,
    recipe_names: &BTreeSet<String>,
) -> CookMode {
    use crate::template::output_pattern_kind_with_recipes;

    if step.using_clause.is_none() {
        return CookMode::DeclarationOnly;
    }

    // Standard §8.4.2 / CS-0089: a parenthesised Lua-expression output is
    // always one-to-one over the recipe's own ingredients. The parser
    // guarantees exactly one output in this form and rejects mixing
    // LuaExpr with Quoted outputs in the same step.
    if step.outputs.iter().any(|p| p.is_lua_expr()) {
        return CookMode::LuaExprOneToOne;
    }

    if step.outputs.len() > 1 {
        let any_iterating = step.outputs.iter().any(|p| {
            matches!(
                output_pattern_kind_with_recipes(p.as_str(), recipe_names),
                OutputPatternKind::OwnInputAccessor | OutputPatternKind::DepDriven { .. }
            )
        });
        return if any_iterating {
            CookMode::OneToMany
        } else {
            CookMode::BlockStep
        };
    }

    match output_pattern_kind_with_recipes(step.outputs[0].as_str(), recipe_names) {
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

/// Convert a `CookMode` to the resolver `IterMode`.
pub(crate) fn cook_mode_to_iter_mode(mode: &CookMode) -> IterMode {
    match mode {
        CookMode::OneToOne | CookMode::OneToMany | CookMode::LuaExprOneToOne => {
            IterMode::OneToOne
        }
        CookMode::ManyToOne | CookMode::BlockStep => IterMode::ManyToOne,
        CookMode::DeclarationOnly => IterMode::OneShot,
    }
}

/// Convert a declared-output count to `OutputShape`.
pub(crate) fn count_to_output_shape(n: usize) -> OutputShape {
    match n {
        0 => OutputShape::None,
        1 => OutputShape::Single,
        n => OutputShape::Multi(n),
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
    let mode = cook_step_mode_with_names(cook_step, recipe_names);
    let iter_mode = cook_mode_to_iter_mode(&mode);
    let output_shape = count_to_output_shape(cook_step.outputs.len());

    // Iteration source per Standard §4.3: the recipe's resolved ingredient
    // set is the union of include globs minus the union of excludes — a
    // single flat list. `recipe.rs` emits that list as the local
    // `ingredients` (via `cook.resolve_ingredients(...)`) at the top of
    // every recipe with ingredients, so we read it here. The
    // `recipe.ingredients[N]` Lua table-of-tables (per-pattern groups)
    // remains available to Lua bodies via the `recipe` global, but is the
    // wrong shape for cook-step iteration — using it here would silently
    // drop every glob past the first.
    let input_source = if let Some(prev) = prev_cook_index {
        format!("_cook_outputs_{}", prev)
    } else if !ingredients.is_empty() {
        "ingredients".to_string()
    } else {
        "{}".to_string()
    };

    // For LuaExpr outputs the pattern source is Lua code, not a sigil
    // template; classifying it as Literal/OwnInputAccessor/DepDriven is
    // meaningless. The dedicated `LuaExprOneToOne` arm below skips this
    // value, so substitute `Literal` defensively.
    let pattern_kind = if cook_step.outputs[0].is_lua_expr() {
        OutputPatternKind::Literal
    } else {
        analyze_output_pattern(cook_step.outputs[0].as_str(), recipe_names)
    };

    match mode {
        CookMode::DeclarationOnly => {
            out.push_str(&format!(
                "    _cook_outputs_{}[1] = \"{}\"\n",
                index,
                crate::lua_string::escape_lua_string(cook_step.outputs[0].as_str())
            ));
        }
        CookMode::LuaExprOneToOne => {
            // Standard §8.4.2 (CS-0089): `cook (EXPR) using ...` — iterate
            // own ingredients, evaluate the parenthesised Lua expression per
            // ingredient with `input` bound to the current ingredient path,
            // and use the resolved string as the unit's single output.
            //
            // The expression text comes straight from the parser; it is a
            // Lua expression evaluated in the Cookfile-scope VM (per §7.1.1
            // coercion rules used by chore default-params).
            let expr_src = cook_step.outputs[0].as_str();

            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                input_source
            ));
            out.push_str("        local _cook_out\n");
            out.push_str("        do\n");
            out.push_str("            local input = _cook_in\n");
            out.push_str(&format!(
                "            _cook_out = ({})\n",
                expr_src
            ));
            out.push_str("        end\n");
            out.push_str("        if type(_cook_out) ~= \"string\" or _cook_out == \"\" then\n");
            out.push_str(
                "            error(\"cook (LUA_EXPR) returned non-string or empty value for input \" .. tostring(_cook_in), 2)\n",
            );
            out.push_str("        end\n");

            let mut consulted = ConsultedEnv::new();
            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let ctx = crate::template::cook_step_ctx(iter_mode, output_shape, recipe_names);
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &ctx, &mut consulted,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table()
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, lua_code = {}, ingredient_groups = {}, consulted_env_keys = \"*\"}})\n",
                        code_literal, ing_groups
                    ));
                }
                None => {
                    unreachable!("LuaExprOneToOne mode requires a using-clause");
                }
            }

            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
            out.push_str("    end\n");
        }
        CookMode::OneToOne => {
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

            let mut consulted = ConsultedEnv::new();
            let out_expr = match &pattern_kind {
                OutputPatternKind::DepDriven { lua_expr, .. } => lua_expr.clone(),
                OutputPatternKind::OwnInputAccessor => {
                    expand_output_pattern(cook_step.outputs[0].as_str(), &mut consulted)
                }
                OutputPatternKind::Literal => {
                    format!("\"{}\"", crate::lua_string::escape_lua_string(cook_step.outputs[0].as_str()))
                }
            };
            out.push_str(&format!("        local _cook_out = {}\n", out_expr));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let ctx = crate::template::cook_step_ctx(iter_mode, output_shape, recipe_names);
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &ctx, &mut consulted,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table()
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, lua_code = {}, ingredient_groups = {}, consulted_env_keys = \"*\"}})\n",
                        code_literal, ing_groups
                    ));
                }
                None => {
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
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \")\n",
                input_source
            ));

            let mut consulted = ConsultedEnv::new();
            let out_expr = match &pattern_kind {
                OutputPatternKind::DepDriven { lua_expr, .. } => lua_expr.clone(),
                OutputPatternKind::OwnInputAccessor => {
                    expand_output_pattern(cook_step.outputs[0].as_str(), &mut consulted)
                }
                OutputPatternKind::Literal => {
                    format!("\"{}\"", crate::lua_string::escape_lua_string(cook_step.outputs[0].as_str()))
                }
            };
            out.push_str(&format!("    local _cook_out = {}\n", out_expr));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let ctx = crate::template::cook_step_ctx(iter_mode, output_shape, recipe_names);
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &ctx, &mut consulted,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = {}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}}})\n",
                        input_source, lua_expr, probes_lua, consulted.to_lua_table()
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = {}, output = _cook_out, lua_code = {}, ingredient_groups = {}, consulted_env_keys = \"*\"}})\n",
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

            let mut consulted = ConsultedEnv::new();
            out.push_str("        local _cook_outs = {\n");
            for pat in &cook_step.outputs {
                let expr = expand_output_pattern(pat.as_str(), &mut consulted);
                out.push_str(&format!("            {},\n", expr));
            }
            out.push_str("        };\n");

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    // OneToMany: multi-output, one-to-one iteration
                    let oto_many_ctx = crate::template::cook_step_ctx(
                        IterMode::OneToOne,
                        OutputShape::Multi(cook_step.outputs.len()),
                        recipe_names,
                    );
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &oto_many_ctx, &mut consulted,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, outputs = _cook_outs, command = {}, probes = {}, consulted_env_keys = {}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table()
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, outputs = _cook_outs, lua_code = {}, ingredient_groups = {}, consulted_env_keys = \"*\"}})\n",
                        code_literal, ing_groups
                    ));
                }
                None => unreachable!("OneToMany mode requires a using-clause"),
            }

            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_outs[1])\n",
                index
            ));
            out.push_str("    end\n");
        }
        CookMode::BlockStep => {
            let mut outs_lua = String::from("{");
            for (i, out_name) in cook_step.outputs.iter().enumerate() {
                if i > 0 {
                    outs_lua.push_str(", ");
                }
                outs_lua.push('"');
                outs_lua.push_str(&crate::lua_string::escape_lua_string(out_name.as_str()));
                outs_lua.push('"');
            }
            outs_lua.push('}');

            out.push_str(&format!("    local _cook_outs = {};\n", outs_lua));
            out.push_str(&format!("    local _cook_ins = {};\n", input_source));
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \");\n",
                input_source
            ));

            match &cook_step.using_clause {
                Some(UsingClause::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let mut consulted = ConsultedEnv::new();
                    // BlockStep: many-to-one with multi outputs
                    let block_ctx = crate::template::cook_step_ctx(
                        IterMode::ManyToOne,
                        OutputShape::Multi(cook_step.outputs.len()),
                        recipe_names,
                    );
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &block_ctx, &mut consulted,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, command = {}, probes = {}, consulted_env_keys = {}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table()
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, lua_code = {}, ingredient_groups = {}, consulted_env_keys = \"*\"}})\n",
                        code_literal, ing_groups
                    ));
                }
                _ => unreachable!("BlockStep mode requires ShellBlock or LuaBlock using-clause"),
            }

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
    use cook_lang::ast::{CookStep, OutputPattern, UsingClause};

    fn step(outputs: &[&str], using_clause: Option<UsingClause>) -> CookStep {
        CookStep {
            outputs: outputs
                .iter()
                .map(|s| OutputPattern::Quoted((*s).to_string()))
                .collect(),
            using_clause,
        }
    }

    fn empty_recipes() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn literal_output_is_many_to_one_regardless_of_body() {
        // A literal output pattern → ManyToOne, even if the body contains $<in>.
        let s = step(
            &["build/app"],
            Some(UsingClause::ShellBlock(vec!["gcc $<in>".into()])),
        );
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::ManyToOne
        ));
    }

    #[test]
    fn in_accessor_output_is_one_to_one() {
        let s = step(
            &["build/$<in.stem>.o"],
            Some(UsingClause::ShellBlock(vec!["gcc $<in> -o $<out>".into()])),
        );
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::OneToOne
        ));
    }

    #[test]
    fn lib_accessor_output_is_one_to_one_dep_driven() {
        // With recipe-name context, `$<libmath.stem>` is recognised as a
        // dep-driven pattern → OneToOne. Without names it is Literal →
        // ManyToOne. Both outcomes are acceptable here; the exhaustive
        // check is in resolves_recipe_accessor / sigil tests.
        let s = step(
            &["build/$<libmath.stem>.x"],
            Some(UsingClause::ShellBlock(vec!["echo $<in>".into()])),
        );
        let mut names = BTreeSet::new();
        names.insert("libmath".to_string());
        assert!(matches!(
            cook_step_mode_with_names(&s, &names),
            CookMode::OneToOne
        ));
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::ManyToOne
        ));
    }

    #[test]
    fn multi_output_literal_is_block_step() {
        let s = step(
            &["a.js", "a.wasm"],
            Some(UsingClause::ShellBlock(vec!["gen".into()])),
        );
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::BlockStep
        ));
    }

    #[test]
    fn declaration_only_no_using_clause() {
        let s = step(&["x"], None);
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::DeclarationOnly
        ));
    }
}
