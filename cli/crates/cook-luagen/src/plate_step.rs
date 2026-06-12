use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::lua_string::lua_chunk_literal;
use crate::template::{
    detect_plate_test_mode, expand_plate_test_body, validate_plate_test_placeholders,
    ConsultedEnv,  PlateTestMode,
};

pub(crate) fn generate_plate_step(
    out: &mut String,
    plate_step: &PlateStep,
    line: usize,
    last_cook_index: Option<usize>,
    has_ingredients: bool,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    let mode = detect_plate_test_mode(&plate_step.body)
        .map_err(|e| CodegenError::PlateTestMode { line, source: e })?;
    validate_plate_test_placeholders(&plate_step.body, mode, recipe_names)
        .map_err(|e| CodegenError::Placeholder { line, source: e })?;

    // CS-0024 §3.5: a OneToOne or ManyToOne plate step requires a non-empty
    // source — either a preceding cook step or at least one declared ingredient
    // glob.  OneShot has no source at all, so the guard does not apply.
    if matches!(mode, PlateTestMode::OneToOne | PlateTestMode::ManyToOne)
        && last_cook_index.is_none()
        && !has_ingredients
    {
        return Err(CodegenError::EmptySource { line });
    }

    // Iteration source per Standard §4.7.1: the preceding `cook` step's
    // output list, or — falling back — the recipe's resolved ingredient
    // set (Standard §4.3 union of includes minus union of excludes).
    // `recipe.rs` emits the resolved set as the local `ingredients`;
    // reading `recipe.ingredients[1]` here would silently drop every
    // glob past the first.
    let source_expr = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "ingredients".to_string()
    };

    // CS-0101: plate steps are cache = false, so `$<file:PATH>` is pure
    // substitution — hoisted locals, but NO `file_refs` unit field.
    let mut file_refs = crate::template::FileRefs::new(format!("l{}", line));

    match (&plate_step.body, mode) {
        // (1) Shell, OneToOne — loop over source, one unit per item.
        (Body::ShellBlock(lines), PlateTestMode::OneToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "_plate_in", "{}", &mut consulted, &mut file_refs);
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n        cook.add_unit({{command = {}, cache = false, consulted_env_keys = {}}})\n    end\n",
                source_expr, cmd_expr, consulted.to_lua_table()
            ));
        }
        // (2) Shell, ManyToOne — one unit, source visible as {all}.
        (Body::ShellBlock(lines), PlateTestMode::ManyToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &source_expr, &mut consulted, &mut file_refs);
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    cook.add_unit({{command = {}, cache = false, consulted_env_keys = {}}})\n",
                cmd_expr, consulted.to_lua_table()
            ));
        }
        // (3) Shell, OneShot — one unit, no source.
        (Body::ShellBlock(lines), PlateTestMode::OneShot) => {
            let cmd_text = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", "{}", &mut consulted, &mut file_refs);
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    cook.add_unit({{command = {}, cache = false, consulted_env_keys = {}}})\n",
                cmd_expr, consulted.to_lua_table()
            ));
        }
        // (4) Lua, OneToOne — loop, body sees `input` as a Lua local.
        //
        // Binding convention (plan §8.1 note): build the `lua_code` string at
        // register time by prepending a header that sets `local input = <value>`
        // using the actual loop-variable value.  We use `string.format("%q", …)`
        // to produce a correctly-quoted Lua string literal, then concatenate with
        // the body long-string.  This way the execute-phase chunk is self-contained
        // and does not rely on any out-of-band `_bind_*` field on `cook.add_unit`.
        //
        // CS-0045: `step_kind = "plate"` tells the worker to run this
        // body without the project-root sandbox — plates are the
        // explicit ship-outside-the-project surface.
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n",
                source_expr
            ));
            out.push_str(&format!(
                "        cook.add_unit({{cache = false, step_kind = \"plate\", lua_code = (\"local input = \" .. string.format(\"%q\", _plate_in) .. \"\\n\") .. {}, consulted_env_keys = \"*\"}})\n",
                lua_chunk_literal(code)
            ));
            out.push_str("    end\n");
        }
        // (5) Lua, ManyToOne — one unit, body sees `inputs` as a Lua local.
        //
        // For many-to-one, the full source list must be available to the body as
        // `inputs`.  We serialise the table at register time using a small Lua
        // helper that quotes each element, producing a self-contained chunk.
        (Body::LuaBlock(code), PlateTestMode::ManyToOne) => {
            out.push_str(&format!(
                "    cook.add_unit({{cache = false, step_kind = \"plate\", lua_code = (function()\n        local _h = {{\"local inputs = {{\"}}\n        for _i, _v in ipairs({}) do if _i > 1 then _h[#_h+1] = \", \" end _h[#_h+1] = string.format(\"%q\", _v) end\n        _h[#_h+1] = \"}}\\n\"\n        return table.concat(_h) .. {}\n    end)(), consulted_env_keys = \"*\"}})\n",
                source_expr, lua_chunk_literal(code)
            ));
        }
        // (6) Lua, OneShot — one unit, no source binding.
        (Body::LuaBlock(code), PlateTestMode::OneShot) => {
            out.push_str(&format!(
                "    cook.add_unit({{cache = false, step_kind = \"plate\", lua_code = {}, consulted_env_keys = \"*\"}})\n",
                lua_chunk_literal(code)
            ));
        }
    }

    // Standard §5.4.1: a `plate` step's output is a passthrough of its
    // input list — independent of how the body uses (or ignores) the
    // source. The passthrough fires whenever there's a meaningful source
    // to forward: a preceding `cook` step's outputs, or the recipe's
    // resolved ingredient set. A recipe with neither has no input list
    // for the plate to pass through, so we skip the call (and the
    // recipe's terminal outputs stay empty).
    if last_cook_index.is_some() || has_ingredients {
        out.push_str(&format!("    cook.passthrough({})\n", source_expr));
    }

    Ok(())
}

/// COOK-63 §8.3: lower a `plate` step inside a `for_each` recipe to one
/// side-effect unit per data member, with the member bound as `item`. A plate
/// declares no output, so there is no passthrough. The recipe body has already
/// emitted `local _items = <source>`.
pub(crate) fn generate_for_each_plate_step(
    out: &mut String,
    plate_step: &PlateStep,
    line: usize,
    recipe_names: &BTreeSet<String>,
) {
    use crate::resolver::{IterMode, OutputShape};
    use crate::template::{cook_step_ctx, expand_for_each_template};

    // OneShot + None: a plate body admits member sigils, recipes, env, and
    // probe refs — but neither `$<in>`/`$<all>` nor `$<out>`.
    let ctx = cook_step_ctx(IterMode::OneShot, OutputShape::None, recipe_names);
    // CS-0101: substitution only (cache = false) — hoists, no file_refs field.
    let mut file_refs = crate::template::FileRefs::new(format!("l{}", line));

    match &plate_step.body {
        Body::ShellBlock(lines) => {
            let combined = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let (cmd_concat, probe_keys) =
                expand_for_each_template(&combined, &ctx, &mut consulted, &mut file_refs).unwrap_or_else(|e| {
                    (
                        format!(
                            "\"[[SIGIL_ERROR: {}]]\"",
                            crate::lua_string::escape_lua_string(&e.to_string())
                        ),
                        BTreeSet::new(),
                    )
                });
            let cmd_expr = if probe_keys.is_empty() {
                cmd_concat
            } else {
                format!("function() return {} end", cmd_concat)
            };
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(&format!(
                "        cook.add_unit({{command = {}, cache = false, consulted_env_keys = {}, member = cook.member_to_string(item)}})\n",
                cmd_expr,
                consulted.to_lua_table()
            ));
            out.push_str("    end\n");
        }
        Body::LuaBlock(code) => {
            // §8.3: the Lua body sees the member as `item`; execute-phase
            // binding of `item` is wired by the COOK-64 runtime slice.
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(&format!(
                "        cook.add_unit({{cache = false, step_kind = \"plate\", lua_code = {}, consulted_env_keys = \"*\", member = cook.member_to_string(item)}})\n",
                lua_chunk_literal(code)
            ));
            out.push_str("    end\n");
        }
    }
}

fn build_shell_block_command(lines: &[String]) -> String {
    let mut s = String::from("set -e");
    for line in lines {
        s.push('\n');
        s.push_str(line);
    }
    s
}

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("plate/test mode error at line {line}: {source}")]
    PlateTestMode {
        line: usize,
        source: crate::template::PlateTestModeError,
    },
    #[error("plate/test placeholder error at line {line}: {source}")]
    Placeholder {
        line: usize,
        source: crate::template::PlateTestPlaceholderError,
    },
    #[error(
        "plate/test step at line {line} requires a non-empty source (one-to-one or \
         many-to-one mode) — add an `ingredients` declaration or a preceding `cook` \
         step (CS-0024 §3.5)"
    )]
    EmptySource { line: usize },
}
