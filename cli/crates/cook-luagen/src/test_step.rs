use cook_lang::ast::*;

use crate::template::expand_test_cmd;

pub(crate) fn generate_test_step(
    out: &mut String,
    test_step: &TestStep,
    _line: usize,
    last_cook_index: Option<usize>,
) {
    let source = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    let cmd_expr = expand_test_cmd(&test_step.command);
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
