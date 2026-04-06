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

    let cmd_expr = expand_plate_cmd_with_deps(&plate_step.command, recipe_names);
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
