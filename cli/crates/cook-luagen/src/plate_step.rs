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
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    let mode = detect_plate_test_mode(&plate_step.body)
        .map_err(|e| CodegenError::PlateTestMode { line, source: e })?;
    validate_plate_test_placeholders(&plate_step.body, mode, recipe_names)
        .map_err(|e| CodegenError::Placeholder { line, source: e })?;

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
        // (4) Lua, OneToOne — loop, body sees `input` via a prepended local.
        // Binding convention: prepend `local input = <item>` to the Lua chunk so
        // the body can reference `input` as the current iteration item.  This
        // matches cook_step.rs's pattern of injecting bindings via a Lua source
        // wrapper rather than an out-of-band `_bind_*` field (cook.add_unit only
        // accepts `inputs`/`outputs`/`lua_code`/`command`/`cache`/`interactive`).
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n",
                source_expr
            ));
            // Wrap: prepend `local input = _plate_in` so the body sees `input`.
            let wrapped_code = format!("local input = _plate_in\n{}", code);
            out.push_str(&format!(
                "        cook.add_unit({{cache = false, lua_code = {}}})\n",
                lua_chunk_literal(&wrapped_code)
            ));
            out.push_str("    end\n");
        }
        // (5) Lua, ManyToOne — one unit, body sees `inputs` via a prepended local.
        (Body::LuaBlock(code), PlateTestMode::ManyToOne) => {
            let wrapped_code = format!("local inputs = {}\n{}", source_expr, code);
            out.push_str(&format!(
                "    cook.add_unit({{cache = false, lua_code = {}}})\n",
                lua_chunk_literal(&wrapped_code)
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
}
