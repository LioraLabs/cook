use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::template::expand_plate_cmd_with_deps;

pub(crate) fn generate_plate_step(
    out: &mut String,
    plate_step: &PlateStep,
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
    let cmd_text = match &plate_step.body {
        cook_lang::ast::UsingClause::ShellBlock(lines) => lines.join("\n"),
        cook_lang::ast::UsingClause::LuaBlock(code) => code.clone(),
    };
    let cmd_expr = expand_plate_cmd_with_deps(&cmd_text, recipe_names);
    out.push_str(&format!(
        "    for _, _plate_out in ipairs({}) do\n",
        source
    ));
    out.push_str(&format!(
        "        cook.add_unit({{command = {}, cache = false}})\n",
        cmd_expr
    ));
    out.push_str("    end\n");
}
