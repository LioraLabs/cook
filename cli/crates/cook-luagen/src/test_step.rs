use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::template::expand_test_cmd_with_deps;

pub(crate) fn generate_test_step(
    out: &mut String,
    test_step: &TestStep,
    _line: usize,
    last_cook_index: Option<usize>,
    recipe_names: &BTreeSet<String>,
) {
    let source = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    // TODO(CS-0024/Task-8): replace with expand_plate_test_body
    let cmd_text = match &test_step.body {
        cook_lang::ast::UsingClause::ShellBlock(lines) => lines.join("\n"),
        cook_lang::ast::UsingClause::LuaBlock(code) => code.clone(),
    };
    let cmd_expr = expand_test_cmd_with_deps(&cmd_text, recipe_names);
    let timeout = test_step.timeout.unwrap_or(300);
    let should_fail = if test_step.should_fail { "true" } else { "false" };

    out.push_str(&format!(
        "    for _, _test_out in ipairs({}) do\n",
        source
    ));
    out.push_str(&format!(
        "        cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n",
        cmd_expr, timeout, should_fail
    ));
    out.push_str("    end\n");
}
