use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::lua_env;
use crate::resolver::{IterMode, OutputShape};
use crate::template::{
    analyze_output_pattern, expand_command_template, expand_output_pattern, ConsultedEnv,
    OutputPatternKind,
};

/// Render the set of Lua-scanned env keys as a `consulted_env_keys` Lua
/// literal. Returns `"{}"` for an empty set (no statically detectable
/// `cook.env.<KEY>` reads).
///
/// Per Standard §17.1, a Lua using-block's cache fingerprint MUST include
/// the values of every env key the body statically reads from `cook.env`.
/// The scanner is in [`lua_env::scan_env_reads`]; this helper threads the
/// scanned keys through the shared [`ConsultedEnv`] accumulator so the
/// rendering path matches the shell-template emission exactly.
fn lua_body_consulted_env_keys(code: &str) -> String {
    let scanned = lua_env::scan_env_reads(code);
    let mut consulted = ConsultedEnv::new();
    for key in &scanned {
        consulted.record(key);
    }
    consulted.to_lua_table()
}

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
/// - No body                             → DeclarationOnly
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

    if step.body.is_none() {
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

/// CS-0101: render the optional `, file_refs = {...}` cook.add_unit field.
/// Empty when no `$<file:PATH>` sigils were seen, so pre-CS-0101 goldens stay
/// byte-identical.
fn file_refs_field(file_refs: &crate::template::FileRefs) -> String {
    if file_refs.is_empty() {
        String::new()
    } else {
        format!(", file_refs = {}", file_refs.to_lua_table())
    }
}

/// COOK-161 + COOK-162 + COOK-163: render the optional `, seal = {...},
/// sharing = "local"|"pinned", record = true` cook.add_unit fields from the
/// step's disposition. Empty when the step declares no seal, the default
/// (`Shared`) sharing, and no `record`, so existing goldens for plain steps stay
/// byte-identical.
///
/// I3: sharing is emitted as a plain string field `sharing = "local"` /
/// `"pinned"` (omitted for `Shared`), which dissolves the old reserved-keyword
/// `["local"]` bracket-quote hack — `sharing` is not a Lua keyword.
fn disposition_field(disp: &Disposition) -> String {
    let mut out = String::new();
    if !disp.seal.is_empty() {
        out.push_str(&format!(", seal = {}", probe_keys_to_lua_table(&disp.seal)));
    }
    if let Some(s) = disp.sharing.as_wire_str() {
        out.push_str(&format!(", sharing = {s:?}"));
    }
    out.push_str(&record_field(disp.record));
    out
}

/// COOK-163: render the optional `, record = true` cook.add_unit field when the
/// step carries the `record` disposition. Empty otherwise so unannotated-step
/// goldens stay byte-identical.
fn record_field(record: bool) -> String {
    if record {
        ", record = true".to_string()
    } else {
        String::new()
    }
}

pub(crate) fn generate_cook_step(
    out: &mut String,
    cook_step: &CookStep,
    line: usize,
    index: usize,
    step_pos: usize,
    prev_cook_index: Option<usize>,
    ingredients: &[String],
    recipe_names: &BTreeSet<String>,
) {
    // CS-0101: one accumulator per cook step, tagged by the step's position in
    // the recipe body so hoisted locals are unique within the recipe chunk.
    let mut file_refs = crate::template::FileRefs::new(format!("s{}", step_pos));
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

            // CS-0101 compute-then-emit: expand the body BEFORE pushing the
            // for-header so file-ref hoists can precede the loop.
            let mut consulted = ConsultedEnv::new();
            let add_unit_line = match &cook_step.body {
                Some(Body::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let ctx = crate::template::cook_step_ctx(iter_mode, output_shape, recipe_names);
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &ctx, &mut consulted, &mut file_refs,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}{}{}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table(), file_refs_field(&file_refs), disposition_field(&cook_step.disposition)
                    )
                }
                Some(Body::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    let env_keys = lua_body_consulted_env_keys(code);
                    format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, lua_code = {}, ingredient_groups = {}, consulted_env_keys = {}{}, line = {}}})\n",
                        code_literal, ing_groups, env_keys, disposition_field(&cook_step.disposition), line
                    )
                }
                None => {
                    unreachable!("LuaExprOneToOne mode requires a using-clause");
                }
            };

            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
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
            out.push_str(&add_unit_line);

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

            // CS-0101 compute-then-emit: expand the output pattern and body
            // BEFORE pushing the for-header so file-ref hoists precede the loop.
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

            let add_unit_line = match &cook_step.body {
                Some(Body::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let ctx = crate::template::cook_step_ctx(iter_mode, output_shape, recipe_names);
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &ctx, &mut consulted, &mut file_refs,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}{}{}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table(), file_refs_field(&file_refs), disposition_field(&cook_step.disposition)
                    )
                }
                Some(Body::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    let env_keys = lua_body_consulted_env_keys(code);
                    format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, lua_code = {}, ingredient_groups = {}, consulted_env_keys = {}{}, line = {}}})\n",
                        code_literal, ing_groups, env_keys, disposition_field(&cook_step.disposition), line
                    )
                }
                None => {
                    unreachable!("OneToOne mode requires a using-clause");
                }
            };

            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                iter_source
            ));
            out.push_str(&format!("        local _cook_out = {}\n", out_expr));
            out.push_str(&add_unit_line);

            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
            out.push_str("    end\n");
        }
        CookMode::ManyToOne => {
            out.push_str(&format!(
                "    local _cook_in = table.concat({}, \" \")\n",
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

            match &cook_step.body {
                Some(Body::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let ctx = crate::template::cook_step_ctx(iter_mode, output_shape, recipe_names);
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &ctx, &mut consulted, &mut file_refs,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    // CS-0101: non-loop step — hoists go right before add_unit.
                    if !file_refs.is_empty() {
                        out.push_str(&file_refs.hoist_lines("    "));
                    }
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = {}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}{}{}}})\n",
                        input_source, lua_expr, probes_lua, consulted.to_lua_table(), file_refs_field(&file_refs), disposition_field(&cook_step.disposition)
                    ));
                }
                Some(Body::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    let env_keys = lua_body_consulted_env_keys(code);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = {}, output = _cook_out, lua_code = {}, ingredient_groups = {}, consulted_env_keys = {}{}, line = {}}})\n",
                        input_source, code_literal, ing_groups, env_keys, disposition_field(&cook_step.disposition), line
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

            // CS-0101 compute-then-emit: expand the output patterns and body
            // BEFORE pushing the for-header so file-ref hoists precede the loop.
            let mut consulted = ConsultedEnv::new();
            let mut outs_block = String::from("        local _cook_outs = {\n");
            for pat in &cook_step.outputs {
                let expr = expand_output_pattern(pat.as_str(), &mut consulted);
                outs_block.push_str(&format!("            {},\n", expr));
            }
            outs_block.push_str("        };\n");

            let add_unit_line = match &cook_step.body {
                Some(Body::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    // OneToMany: multi-output, one-to-one iteration
                    let oto_many_ctx = crate::template::cook_step_ctx(
                        IterMode::OneToOne,
                        OutputShape::Multi(cook_step.outputs.len()),
                        recipe_names,
                    );
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &oto_many_ctx, &mut consulted, &mut file_refs,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, outputs = _cook_outs, command = {}, probes = {}, consulted_env_keys = {}{}{}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table(), file_refs_field(&file_refs), disposition_field(&cook_step.disposition)
                    )
                }
                Some(Body::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    let env_keys = lua_body_consulted_env_keys(code);
                    format!(
                        "        cook.add_unit({{inputs = {{_cook_in}}, outputs = _cook_outs, lua_code = {}, ingredient_groups = {}, consulted_env_keys = {}{}, line = {}}})\n",
                        code_literal, ing_groups, env_keys, disposition_field(&cook_step.disposition), line
                    )
                }
                None => unreachable!("OneToMany mode requires a using-clause"),
            };

            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                iter_source
            ));
            out.push_str(&outs_block);
            out.push_str(&add_unit_line);

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
                "    local _cook_in = table.concat({}, \" \");\n",
                input_source
            ));

            match &cook_step.body {
                Some(Body::ShellBlock(lines)) => {
                    let combined = build_shell_block_command(lines, recipe_names);
                    let mut consulted = ConsultedEnv::new();
                    // BlockStep: many-to-one with multi outputs
                    let block_ctx = crate::template::cook_step_ctx(
                        IterMode::ManyToOne,
                        OutputShape::Multi(cook_step.outputs.len()),
                        recipe_names,
                    );
                    let (lua_expr, probe_keys) = match expand_command_template(
                        &combined, &block_ctx, &mut consulted, &mut file_refs,
                    ) {
                        Ok(pair) => pair,
                        Err(e) => (
                            format!("\"[[SIGIL_ERROR: {}]]\"", crate::lua_string::escape_lua_string(&e.to_string())),
                            std::collections::BTreeSet::new(),
                        ),
                    };
                    let probes_lua = probe_keys_to_lua_table(&probe_keys);
                    // CS-0101: non-loop step — hoists go right before add_unit.
                    if !file_refs.is_empty() {
                        out.push_str(&file_refs.hoist_lines("    "));
                    }
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, command = {}, probes = {}, consulted_env_keys = {}{}{}}})\n",
                        lua_expr, probes_lua, consulted.to_lua_table(), file_refs_field(&file_refs), disposition_field(&cook_step.disposition)
                    ));
                }
                Some(Body::LuaBlock(code)) => {
                    let code_literal = crate::lua_string::wrap_lua_string(code);
                    let ing_groups = format_ingredient_groups(ingredients.len());
                    let env_keys = lua_body_consulted_env_keys(code);
                    out.push_str(&format!(
                        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, lua_code = {}, ingredient_groups = {}, consulted_env_keys = {}{}, line = {}}})\n",
                        code_literal, ing_groups, env_keys, disposition_field(&cook_step.disposition), line
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

/// COOK-63 §8.3: lower a `cook` step inside a `for_each` recipe to one
/// `cook.add_unit` per data member, with the member bound as `item`.
///
/// The recipe body has already emitted `local _items = <source>` (see
/// `recipe::generate_with_names`); this emits the per-member loop, wrapped by
/// the caller's `cook.step_group` and preceded by `local _cook_outputs_N = {}`.
/// `$<in>` / `$<in.FIELD>` resolve against the loop's `item` member; `$<out>`
/// resolves to the member's declared output. The filesystem path-input
/// builtin (`$<in>`'s glob-path sense) is not applicable in a
/// `for_each` body — it has no path-input source. The probe-deferral and Lua
/// long-string conventions mirror the ingredient-driven `LuaExprOneToOne` arm.
pub(crate) fn generate_for_each_cook_step(
    out: &mut String,
    cook_step: &CookStep,
    index: usize,
    step_pos: usize,
    recipe_names: &BTreeSet<String>,
) {
    // CS-0101: per-step accumulator; hoists are emitted once, OUTSIDE the
    // member loop, so a file ref resolves once per step (not per member).
    let mut file_refs = crate::template::FileRefs::new(format!("s{}", step_pos));
    // OneShot rejects `$<in>`; Single permits the lone `$<out>`.
    let ctx = crate::template::cook_step_ctx(IterMode::OneShot, OutputShape::Single, recipe_names);
    let sigil_err = |e: crate::resolver::ResolveError| {
        (
            format!(
                "\"[[SIGIL_ERROR: {}]]\"",
                crate::lua_string::escape_lua_string(&e.to_string())
            ),
            BTreeSet::new(),
        )
    };

    // CS-0101 compute-then-emit: expand the output path and body BEFORE
    // pushing the member-loop header so file-ref hoists precede the loop.
    //
    // Output path: member sigils + literals, no probe-deferral (the output
    // path is a register-time value).
    let mut consulted = ConsultedEnv::new();
    let (out_expr, _) = crate::template::expand_for_each_template(
        cook_step.outputs[0].as_str(),
        &ctx,
        &mut consulted,
        &mut file_refs,
        crate::template::ProbeLowering::CacheGet,
    )
    .unwrap_or_else(sigil_err);

    let add_unit_line = match &cook_step.body {
        Some(Body::ShellBlock(lines)) => {
            let combined = build_shell_block_command(lines, recipe_names);
            let (cmd_concat, probe_keys) = crate::template::expand_for_each_template(
                &combined,
                &ctx,
                &mut consulted,
                &mut file_refs,
                crate::template::ProbeLowering::LiteralSigil,
            )
            .unwrap_or_else(sigil_err);
            // COOK-187 / CS-0122: probe refs stay literal sigil text in the
            // command string — never a deferred function (see
            // expand_command_template's doc comment for the rationale).
            let probes_lua = probe_keys_to_lua_table(&probe_keys);
            format!(
                "        cook.add_unit({{inputs = {{}}, output = _cook_out, command = {}, probes = {}, consulted_env_keys = {}{}, member = cook.member_to_string(item){}}})\n",
                cmd_concat, probes_lua, consulted.to_lua_table(), file_refs_field(&file_refs), disposition_field(&cook_step.disposition)
            )
        }
        Some(Body::LuaBlock(code)) => {
            // §8.3: a Lua block body sees the member as `item`. Execute-phase
            // binding of `item` is wired by the COOK-64 runtime slice.
            let code_literal = crate::lua_string::wrap_lua_string(code);
            let env_keys = lua_body_consulted_env_keys(code);
            format!(
                "        cook.add_unit({{inputs = {{}}, output = _cook_out, lua_code = {}, consulted_env_keys = {}, member = cook.member_to_string(item){}}})\n",
                code_literal, env_keys, disposition_field(&cook_step.disposition)
            )
        }
        None => {
            // Declaration-only: one declared output per member, no command.
            format!(
                "        cook.add_unit({{inputs = {{}}, output = _cook_out, member = cook.member_to_string(item){}}})\n",
                disposition_field(&cook_step.disposition)
            )
        }
    };

    if !file_refs.is_empty() {
        out.push_str(&file_refs.hoist_lines("    "));
    }
    out.push_str("    for _, item in ipairs(_items) do\n");
    out.push_str(&format!("        local _cook_out = {}\n", out_expr));
    out.push_str(&add_unit_line);

    out.push_str(&format!(
        "        table.insert(_cook_outputs_{}, _cook_out)\n",
        index
    ));
    out.push_str("    end\n");
}

#[cfg(test)]
mod cs_0022_mode_tests {
    use super::*;
    use cook_lang::ast::{CookStep, OutputPattern, Body};

    fn step(outputs: &[&str], body: Option<Body>) -> CookStep {
        CookStep {
            outputs: outputs
                .iter()
                .map(|s| OutputPattern::Quoted((*s).to_string()))
                .collect(),
            body,
            disposition: Default::default(),
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
            Some(Body::ShellBlock(vec!["gcc $<in>".into()])),
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
            Some(Body::ShellBlock(vec!["gcc $<in> -o $<out>".into()])),
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
            Some(Body::ShellBlock(vec!["echo $<in>".into()])),
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
            Some(Body::ShellBlock(vec!["gen".into()])),
        );
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::BlockStep
        ));
    }

    #[test]
    fn declaration_only_no_body() {
        let s = step(&["x"], None);
        assert!(matches!(
            cook_step_mode_with_names(&s, &empty_recipes()),
            CookMode::DeclarationOnly
        ));
    }
}

#[cfg(test)]
mod cook_162_disposition_field_tests {
    use super::*;

    #[test]
    fn disposition_field_emits_local_and_pinned() {
        // I3: sharing is a plain string field (no reserved-keyword hack).
        let mut d = Disposition::default();
        d.sharing = cook_contracts::Sharing::Local;
        assert_eq!(disposition_field(&d), ", sharing = \"local\"");
        let mut d2 = Disposition::default();
        d2.sharing = cook_contracts::Sharing::Pinned;
        assert_eq!(disposition_field(&d2), ", sharing = \"pinned\"");
        let mut d3 = Disposition::default();
        d3.seal.insert("host".to_string());
        assert!(disposition_field(&d3).contains("seal = "));
        assert_eq!(disposition_field(&Disposition::default()), "");
    }
}
