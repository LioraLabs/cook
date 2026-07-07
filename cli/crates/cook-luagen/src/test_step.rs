use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::lua_string::lua_chunk_literal;
use crate::plate_step::CodegenError;
use crate::template::{
    detect_plate_test_mode, expand_plate_test_body, validate_plate_test_placeholders,
    ConsultedEnv, PlateTestMode,
};

pub(crate) fn generate_test_step(
    out: &mut String,
    test_step: &TestStep,
    line: usize,
    last_cook_index: Option<usize>,
    has_ingredients: bool,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    let mode = detect_plate_test_mode(&test_step.body)
        .map_err(|e| CodegenError::PlateTestMode { line, source: e })?;
    validate_plate_test_placeholders(&test_step.body, mode, recipe_names)
        .map_err(|e| CodegenError::Placeholder { line, source: e })?;

    // CS-0024 §3.5: a OneToOne or ManyToOne test step requires a non-empty
    // source — either a preceding cook step or at least one declared ingredient
    // glob.  OneShot has no source at all, so the guard does not apply.
    if matches!(mode, PlateTestMode::OneToOne | PlateTestMode::ManyToOne)
        && last_cook_index.is_none()
        && !has_ingredients
    {
        return Err(CodegenError::EmptySource { line });
    }

    // Iteration source per Standard §4.8.1: same fallback chain as
    // plate (preceding cook step's outputs, or the resolved ingredient
    // set). The `ingredients` local emitted by `recipe.rs` carries the
    // §4.3 union; `recipe.ingredients[1]` would drop globs 2..N.
    let source_expr = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "ingredients".to_string()
    };
    let timeout = test_step.timeout.unwrap_or(300);
    let should_fail = if test_step.should_fail { "true" } else { "false" };

    // COOK-84: when the recipe declares ingredients, every add_test in this
    // step carries the resolved ingredient file list so the engine can fold
    // the files' content into the test fingerprint (§17.4.1). Tests sourced
    // solely from a preceding cook step carry no inputs — the cook step's
    // own cache_meta.input_paths are folded via the predecessor closure in
    // cook-engine/src/run.rs.
    let inputs_field: &str = if has_ingredients { "inputs = ingredients, " } else { "" };

    // CS-0101: test steps are cache = false, so `$<file:PATH>` is pure
    // substitution — hoisted locals, but NO `file_refs` unit field. The
    // accumulator is shared by the as-name and body expansions (patterns
    // dedupe), and hoists are emitted before the first unit line of each arm.
    let mut file_refs = crate::template::FileRefs::new(format!("l{}", line));

    // Expand as_name using the same sigil machinery as the body, but with
    // bindings appropriate to each mode.  The result is a Lua string
    // expression (e.g. `"non-empty"` or `path.stem(_test_in) .. "-rt"`).
    // OneShot / ManyToOne use an empty iter binding; OneToOne uses _test_in.
    let as_name_oto = test_step.as_name.as_deref().map(|n| {
        let mut _ignored = ConsultedEnv::new();
        expand_plate_test_body(n, recipe_names, "_test_in", "{}", &mut _ignored, &mut file_refs)
    });
    let as_name_mto = test_step.as_name.as_deref().map(|n| {
        let mut _ignored = ConsultedEnv::new();
        expand_plate_test_body(n, recipe_names, "\"\"", &source_expr, &mut _ignored, &mut file_refs)
    });
    let as_name_oneshot = test_step.as_name.as_deref().map(|n| {
        let mut _ignored = ConsultedEnv::new();
        expand_plate_test_body(n, recipe_names, "\"\"", "{}", &mut _ignored, &mut file_refs)
    });

    match (&test_step.body, mode) {
        (Body::ShellBlock(lines), PlateTestMode::OneToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "_test_in", "{}", &mut consulted, &mut file_refs);
            let name_field = fmt_name_field(as_name_oto.as_deref());
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    for _, _test_in in ipairs({}) do\n        cook.add_test({{command = {}, {}{}timeout = {}, should_fail = {}, line = {}, iteration_item = _test_in, consulted_env_keys = {}}})\n    end\n",
                source_expr, cmd_expr, inputs_field, name_field, timeout, should_fail, line, consulted.to_lua_table()
            ));
        }
        (Body::ShellBlock(lines), PlateTestMode::ManyToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &source_expr, &mut consulted, &mut file_refs);
            let name_field = fmt_name_field(as_name_mto.as_deref());
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    cook.add_test({{command = {}, {}{}timeout = {}, should_fail = {}, line = {}, iteration_item = nil, consulted_env_keys = {}}})\n",
                cmd_expr, inputs_field, name_field, timeout, should_fail, line, consulted.to_lua_table()
            ));
        }
        (Body::ShellBlock(lines), PlateTestMode::OneShot) => {
            let cmd_text = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", "{}", &mut consulted, &mut file_refs);
            let name_field = fmt_name_field(as_name_oneshot.as_deref());
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    cook.add_test({{command = {}, {}{}timeout = {}, should_fail = {}, line = {}, iteration_item = nil, consulted_env_keys = {}}})\n",
                cmd_expr, inputs_field, name_field, timeout, should_fail, line, consulted.to_lua_table()
            ));
        }
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            let name_field = fmt_name_field(as_name_oto.as_deref());
            // CS-0101: the as-name expression may reference a file-ref local.
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    for _, _test_in in ipairs({}) do\n",
                source_expr
            ));
            // Binding convention: build lua_code at register time with `local input = <value>`.
            out.push_str(&format!(
                "        cook.add_test({{lua_code = (\"local input = \" .. string.format(\"%q\", _test_in) .. \"\\n\") .. {}, {}{}timeout = {}, should_fail = {}, line = {}, iteration_item = _test_in, consulted_env_keys = \"*\"}})\n",
                lua_chunk_literal(code), inputs_field, name_field, timeout, should_fail, line
            ));
            out.push_str("    end\n");
        }
        (Body::LuaBlock(code), PlateTestMode::ManyToOne) => {
            let name_field = fmt_name_field(as_name_mto.as_deref());
            // CS-0101: the as-name expression may reference a file-ref local.
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            // Serialise the inputs table at register time into the lua_code string.
            out.push_str(&format!(
                "    cook.add_test({{lua_code = (function()\n        local _h = {{\"local inputs = {{\"}}\n        for _i, _v in ipairs({}) do if _i > 1 then _h[#_h+1] = \", \" end _h[#_h+1] = string.format(\"%q\", _v) end\n        _h[#_h+1] = \"}}\\n\"\n        return table.concat(_h) .. {}\n    end)(), {}{}timeout = {}, should_fail = {}, line = {}, iteration_item = nil, consulted_env_keys = \"*\"}})\n",
                source_expr, lua_chunk_literal(code), inputs_field, name_field, timeout, should_fail, line
            ));
        }
        (Body::LuaBlock(code), PlateTestMode::OneShot) => {
            let name_field = fmt_name_field(as_name_oneshot.as_deref());
            // CS-0101: the as-name expression may reference a file-ref local.
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str(&format!(
                "    cook.add_test({{lua_code = {}, {}{}timeout = {}, should_fail = {}, line = {}, iteration_item = nil, consulted_env_keys = \"*\"}})\n",
                lua_chunk_literal(code), inputs_field, name_field, timeout, should_fail, line
            ));
        }
    }

    // Standard §5.4.1: a `test` step's output is a passthrough of its
    // input list — independent of how the body uses (or ignores) the
    // source. Same source-availability guard as plate.
    if last_cook_index.is_some() || has_ingredients {
        out.push_str(&format!("    cook.passthrough({})\n", source_expr));
    }

    Ok(())
}

/// COOK-63 §8.3: lower a `test` step inside a `for_each` recipe to one test
/// unit per data member, with the member bound as `item`. The recipe body has
/// already emitted `local _items = <source>`.
pub(crate) fn generate_for_each_test_step(
    out: &mut String,
    test_step: &TestStep,
    line: usize,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    use crate::resolver::{IterMode, OutputShape};
    use crate::template::{cook_step_ctx, expand_for_each_template};

    let ctx = cook_step_ctx(IterMode::OneShot, OutputShape::None, recipe_names);
    let timeout = test_step.timeout.unwrap_or(300);
    let should_fail = if test_step.should_fail { "true" } else { "false" };
    // CS-0101: substitution only (cache = false) — hoists, no file_refs field.
    let mut file_refs = crate::template::FileRefs::new(format!("l{}", line));

    // `as` name is member-aware too; it is evaluated per-iteration (it may
    // reference `item`).
    let name_field = match test_step.as_name.as_deref() {
        Some(n) => {
            let mut _ignored = ConsultedEnv::new();
            let (expr, _) = expand_for_each_template(
                n,
                &ctx,
                &mut _ignored,
                &mut file_refs,
                crate::template::ProbeLowering::CacheGet,
            )
            .map_err(|source| CodegenError::SigilResolve { line, source })?;
            format!("name = {}, ", expr)
        }
        None => String::new(),
    };

    match &test_step.body {
        Body::ShellBlock(lines) => {
            let combined = build_shell_block_command(lines);
            let mut consulted = ConsultedEnv::new();
            let (cmd_concat, probe_keys) = expand_for_each_template(
                &combined,
                &ctx,
                &mut consulted,
                &mut file_refs,
                crate::template::ProbeLowering::CacheGet,
            )
            .map_err(|source| CodegenError::SigilResolve { line, source })?;
            // CS-0127: `WorkPayload::Test` runs `cmd` verbatim via `/bin/sh`
            // — there is no execute-phase probe-substitution machinery for
            // test commands the way `cook.add_unit` rewrites a probe-bearing
            // command into a LuaChunk (unit_api.rs's
            // `try_expand_probe_templates`). The old codegen wrapped the
            // command in `function() return ... end`, but that closure now
            // hard-errors at `cook.add_test` register time (CS-0127's
            // strict `command` typing) instead of silently degrading. Fail
            // loudly at codegen time instead, with an actionable fix.
            if !probe_keys.is_empty() {
                // BTreeSet already yields keys in sorted order.
                let keys: Vec<String> = probe_keys.into_iter().collect();
                return Err(CodegenError::ProbeRefInTestCommand { line, keys });
            }
            let cmd_expr = cmd_concat;
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(&format!(
                "        cook.add_test({{command = {}, {}timeout = {}, should_fail = {}, line = {}, iteration_item = cook.member_to_string(item), consulted_env_keys = {}, member = cook.member_to_string(item)}})\n",
                cmd_expr, name_field, timeout, should_fail, line, consulted.to_lua_table()
            ));
            out.push_str("    end\n");
        }
        Body::LuaBlock(code) => {
            // §8.3: the Lua body sees the member as `item`; execute-phase
            // binding of `item` is wired by the COOK-64 runtime slice.
            // CS-0101: the as-name expression may reference a file-ref local.
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(&format!(
                "        cook.add_test({{lua_code = {}, {}timeout = {}, should_fail = {}, line = {}, iteration_item = cook.member_to_string(item), consulted_env_keys = \"*\", member = cook.member_to_string(item)}})\n",
                lua_chunk_literal(code), name_field, timeout, should_fail, line
            ));
            out.push_str("    end\n");
        }
    }
    Ok(())
}

/// Format the `name = <expr>, ` fragment for cook.add_test tables.
/// Returns an empty string when no as_name was present.
fn fmt_name_field(as_name_expr: Option<&str>) -> String {
    match as_name_expr {
        Some(expr) => format!("name = {}, ", expr),
        None => String::new(),
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
