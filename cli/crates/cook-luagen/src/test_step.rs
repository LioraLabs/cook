use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::lua_string::lua_chunk_literal;
use crate::template::{
    detect_plate_test_mode, expand_plate_test_body, validate_plate_test_placeholders,
    ConsultedEnv, PlateTestMode,
};

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("test mode error at line {line}: {source}")]
    PlateTestMode {
        line: usize,
        source: crate::template::PlateTestModeError,
    },
    #[error("test placeholder error at line {line}: {source}")]
    Placeholder {
        line: usize,
        source: crate::template::PlateTestPlaceholderError,
    },
    #[error(
        "test step at line {line} requires a non-empty source (one-to-one or \
         many-to-one mode) — add an `ingredients` declaration or a preceding `cook` \
         step (CS-0024 §3.5)"
    )]
    EmptySource { line: usize },
    #[error("probe-value reference(s) {keys:?} in a `test` shell command inside a `for_each` recipe at line {line} are not supported — read the probe value in a Lua test body (`test >{{ ... }}`) instead (CS-0127)")]
    ProbeRefInTestCommand { line: usize, keys: Vec<String> },
    #[error("test placeholder error at line {line}: {source}")]
    SigilResolve {
        line: usize,
        source: crate::resolver::ResolveError,
    },
}

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

    // COOK-84: when the recipe declares ingredients, every add_test in this
    // step carries the resolved ingredient file list so the engine can fold
    // the files' content into the test fingerprint (§17.4.1). Tests sourced
    // solely from a preceding cook step carry no inputs — the cook step's
    // own cache_meta.input_paths are folded via the predecessor closure in
    // cook-engine/src/run.rs.
    let inputs_field: &str = if has_ingredients { "inputs = ingredients, " } else { "" };

    // CS-0159: the test unit's effective seal set (recipe baseline folded with
    // this step's trailing seal/unseal by the parser). Emitted as a leading
    // `seal = {...}, ` field so every add_test arm below carries it uniformly;
    // empty when the test seals nothing, keeping existing goldens for
    // unsealed tests byte-identical.
    let seal_field: String = if test_step.seal.is_empty() {
        String::new()
    } else {
        format!(
            "seal = {}, ",
            crate::cook_step::probe_keys_to_lua_table(&test_step.seal)
        )
    };
    let seal_field: &str = &seal_field;

    // CS-0101: test steps are cache = false, so `$<file:PATH>` is pure
    // substitution — hoisted locals, but NO `file_refs` unit field. The
    // accumulator is shared by the as-name and body expansions (patterns
    // dedupe), and hoists are emitted before the first unit line of each arm.
    let mut file_refs = crate::template::FileRefs::new(format!("l{}", line));

    match &test_step.body {
        Body::ShellBlock(lines) => match mode {
            PlateTestMode::OneToOne => {
                let cmd_text = build_shell_block_command(lines);
                let mut consulted = ConsultedEnv::new();
                let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "_test_in", &mut consulted, &mut file_refs);
                if !file_refs.is_empty() {
                    out.push_str(&file_refs.hoist_lines("    "));
                }
                out.push_str(&format!(
                    "    for _, _test_in in ipairs({}) do\n        cook.add_test({{command = {}, {}{}line = {}, iteration_item = _test_in, consulted_env_keys = {}}})\n    end\n",
                    source_expr, cmd_expr, inputs_field, seal_field, line, consulted.to_lua_table()
                ));
            }
            // `ManyToOne` is grouped in here for exhaustiveness only:
            // `detect_plate_test_mode` never returns it for `Body::ShellBlock`
            // (CS-0130 removed `$<all>`, so a batched test is
            // Lua-block-only), and this arm is otherwise byte-for-byte what
            // the old OneShot arm did.
            PlateTestMode::OneShot | PlateTestMode::ManyToOne => {
                let cmd_text = build_shell_block_command(lines);
                let mut consulted = ConsultedEnv::new();
                let cmd_expr = expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &mut consulted, &mut file_refs);
                if !file_refs.is_empty() {
                    out.push_str(&file_refs.hoist_lines("    "));
                }
                out.push_str(&format!(
                    "    cook.add_test({{command = {}, {}{}line = {}, iteration_item = nil, consulted_env_keys = {}}})\n",
                    cmd_expr, inputs_field, seal_field, line, consulted.to_lua_table()
                ));
            }
        },
        Body::LuaBlock(code) => match mode {
            PlateTestMode::OneToOne => {
                if !file_refs.is_empty() {
                    out.push_str(&file_refs.hoist_lines("    "));
                }
                out.push_str(&format!(
                    "    for _, _test_in in ipairs({}) do\n",
                    source_expr
                ));
                // Binding convention: build lua_code at register time with `local input = <value>`.
                out.push_str(&format!(
                    "        cook.add_test({{lua_code = (\"local input = \" .. string.format(\"%q\", _test_in) .. \"\\n\") .. {}, {}{}line = {}, iteration_item = _test_in, consulted_env_keys = \"*\"}})\n",
                    lua_chunk_literal(code), inputs_field, seal_field, line
                ));
                out.push_str("    end\n");
            }
            PlateTestMode::ManyToOne => {
                if !file_refs.is_empty() {
                    out.push_str(&file_refs.hoist_lines("    "));
                }
                // Serialise the inputs table at register time into the lua_code string.
                out.push_str(&format!(
                    "    cook.add_test({{lua_code = (function()\n        local _h = {{\"local inputs = {{\"}}\n        for _i, _v in ipairs({}) do if _i > 1 then _h[#_h+1] = \", \" end _h[#_h+1] = string.format(\"%q\", _v) end\n        _h[#_h+1] = \"}}\\n\"\n        return table.concat(_h) .. {}\n    end)(), {}{}line = {}, iteration_item = nil, consulted_env_keys = \"*\"}})\n",
                    source_expr, lua_chunk_literal(code), inputs_field, seal_field, line
                ));
            }
            PlateTestMode::OneShot => {
                if !file_refs.is_empty() {
                    out.push_str(&file_refs.hoist_lines("    "));
                }
                out.push_str(&format!(
                    "    cook.add_test({{lua_code = {}, {}{}line = {}, iteration_item = nil, consulted_env_keys = \"*\"}})\n",
                    lua_chunk_literal(code), inputs_field, seal_field, line
                ));
            }
        },
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
    // CS-0159: the effective seal set travels with the fan-out units too —
    // every member unit of a sealed `test` keys on the same sealed probes.
    let seal_field: String = if test_step.seal.is_empty() {
        String::new()
    } else {
        format!("seal = {}, ", crate::cook_step::probe_keys_to_lua_table(&test_step.seal))
    };
    let seal_field: &str = &seal_field;
    // CS-0101: substitution only (cache = false) — hoists, no file_refs field.
    let mut file_refs = crate::template::FileRefs::new(format!("l{}", line));

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
                "        cook.add_test({{command = {}, {}line = {}, iteration_item = cook.member_to_string(item), consulted_env_keys = {}, member = cook.member_to_string(item)}})\n",
                cmd_expr, seal_field, line, consulted.to_lua_table()
            ));
            out.push_str("    end\n");
        }
        Body::LuaBlock(code) => {
            // §8.3: the Lua body sees the member as `item`; execute-phase
            // binding of `item` is wired by the COOK-64 runtime slice.
            if !file_refs.is_empty() {
                out.push_str(&file_refs.hoist_lines("    "));
            }
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(&format!(
                "        cook.add_test({{lua_code = {}, {}line = {}, iteration_item = cook.member_to_string(item), consulted_env_keys = \"*\", member = cook.member_to_string(item)}})\n",
                lua_chunk_literal(code), seal_field, line
            ));
            out.push_str("    end\n");
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
