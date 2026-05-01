use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::plate_step::CodegenError;
use crate::template::{
    detect_plate_test_mode, expand_plate_test_body, validate_plate_test_placeholders, PlateTestMode,
};

pub(crate) fn generate_test_step(
    out: &mut String,
    test_step: &TestStep,
    line: usize,
    last_cook_index: Option<usize>,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    let mode = detect_plate_test_mode(&test_step.body)
        .map_err(|e| CodegenError::PlateTestMode { line, source: e })?;
    validate_plate_test_placeholders(&test_step.body, mode, recipe_names)
        .map_err(|e| CodegenError::Placeholder { line, source: e })?;

    let source_expr = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };
    let timeout = test_step.timeout.unwrap_or(300);
    let should_fail = if test_step.should_fail { "true" } else { "false" };

    match (&test_step.body, mode) {
        (Body::ShellBlock(lines), PlateTestMode::OneToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "_test_in", "{}");
            out.push_str(&format!(
                "    for _, _test_in in ipairs({}) do\n        cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n    end\n",
                source_expr, cmd_expr, timeout, should_fail
            ));
        }
        (Body::ShellBlock(lines), PlateTestMode::ManyToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &source_expr);
            out.push_str(&format!(
                "    cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n",
                cmd_expr, timeout, should_fail
            ));
        }
        (Body::ShellBlock(lines), PlateTestMode::OneShot) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", "{}");
            out.push_str(&format!(
                "    cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n",
                cmd_expr, timeout, should_fail
            ));
        }
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            out.push_str(&format!(
                "    for _, _test_in in ipairs({}) do\n",
                source_expr
            ));
            // Wrap: prepend `local input = _test_in` so the body sees `input`.
            let wrapped_code = format!("local input = _test_in\n{}", code);
            out.push_str(&format!(
                "        cook.add_test({{lua_code = {}, timeout = {}, should_fail = {}}})\n",
                lua_chunk_literal(&wrapped_code), timeout, should_fail
            ));
            out.push_str("    end\n");
        }
        (Body::LuaBlock(code), PlateTestMode::ManyToOne) => {
            let wrapped_code = format!("local inputs = {}\n{}", source_expr, code);
            out.push_str(&format!(
                "    cook.add_test({{lua_code = {}, timeout = {}, should_fail = {}}})\n",
                lua_chunk_literal(&wrapped_code), timeout, should_fail
            ));
        }
        (Body::LuaBlock(code), PlateTestMode::OneShot) => {
            out.push_str(&format!(
                "    cook.add_test({{lua_code = {}, timeout = {}, should_fail = {}}})\n",
                lua_chunk_literal(code), timeout, should_fail
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
