use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::template::{
    detect_plate_test_mode, expand_plate_test_body, validate_plate_test_placeholders, PlateTestMode,
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

    // CS-0024 §3.5: a OneToOne plate step requires a non-empty source — either
    // a preceding cook step or at least one declared ingredient glob.
    if mode == PlateTestMode::OneToOne && last_cook_index.is_none() && !has_ingredients {
        return Err(CodegenError::EmptySource { line });
    }

    let source_expr = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    match (&plate_step.body, mode) {
        // (1) Shell, OneToOne — loop over source, one unit per item.
        (Body::ShellBlock(lines), PlateTestMode::OneToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "_plate_in", "{}");
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n        cook.add_unit({{command = {}, cache = false}})\n    end\n",
                source_expr, cmd_expr
            ));
        }
        // (2) Shell, ManyToOne — one unit, source visible as {all}.
        (Body::ShellBlock(lines), PlateTestMode::ManyToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &source_expr);
            out.push_str(&format!(
                "    cook.add_unit({{command = {}, cache = false}})\n",
                cmd_expr
            ));
        }
        // (3) Shell, OneShot — one unit, no source.
        (Body::ShellBlock(lines), PlateTestMode::OneShot) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", "{}");
            out.push_str(&format!(
                "    cook.add_unit({{command = {}, cache = false}})\n",
                cmd_expr
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
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n",
                source_expr
            ));
            out.push_str(&format!(
                "        cook.add_unit({{cache = false, lua_code = (\"local input = \" .. string.format(\"%q\", _plate_in) .. \"\\n\") .. {}}})\n",
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
                "    cook.add_unit({{cache = false, lua_code = (function()\n        local _h = {{\"local inputs = {{\"}}\n        for _i, _v in ipairs({}) do if _i > 1 then _h[#_h+1] = \", \" end _h[#_h+1] = string.format(\"%q\", _v) end\n        _h[#_h+1] = \"}}\\n\"\n        return table.concat(_h) .. {}\n    end)()}})\n",
                source_expr, lua_chunk_literal(code)
            ));
        }
        // (6) Lua, OneShot — one unit, no source binding.
        (Body::LuaBlock(code), PlateTestMode::OneShot) => {
            out.push_str(&format!(
                "    cook.add_unit({{cache = false, lua_code = {}}})\n",
                lua_chunk_literal(code)
            ));
        }
    }

    Ok(())
}

fn build_shell_block_command(lines: &[String]) -> String {
    let mut s = String::from("set -e");
    for line in lines {
        s.push('\n');
        s.push_str(line);
    }
    s
}

fn lua_chunk_literal(code: &str) -> String {
    format!("[==[\n{}\n]==]", code)
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
        "plate step at line {line} is one-to-one but has no non-empty source — \
         add an `ingredients` declaration or a preceding `cook` step (CS-0024 §3.5)"
    )]
    EmptySource { line: usize },
}
