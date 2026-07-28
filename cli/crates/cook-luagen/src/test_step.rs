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
    /// CS-0127 / COOK-357. A test unit's `cmd` runs verbatim via `/bin/sh`;
    /// unlike `cook.add_unit`, nothing rewrites a probe-bearing test command
    /// into an execute-time `cook.probes.get` chunk (see COOK-360, which
    /// removes the distinction and with it this error).
    ///
    /// Both test paths raise this one error. The fan-out path always did; the
    /// plain path instead let the reference fall through to the variable
    /// lookup and reported `no config block declares '<key>'`, advice that
    /// could not work. Routing the plain path through the shared resolver
    /// without this arm would substitute at REGISTER time, where the probe is
    /// unmaterialised — a nil read dressed up as a value.
    #[error(
        "probe-value reference(s) {keys:?} in a `test` shell command at line {line} \
         are not supported — a test command runs verbatim, with no execute-time \
         probe substitution. Read the value in a Lua test body \
         (`test >{{ cook.probes.get(\"<key>\") ... }}`) instead (CS-0127)"
    )]
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

    // Iteration source per §8.6.1: the preceding step's outputs, falling back
    // to the resolved ingredient set. The `ingredients` local emitted by
    // `recipe.rs` carries the §4.3 union; `recipe.ingredients[1]` would drop
    // globs 2..N.
    //
    // The chain is evaluated at REGISTER phase, through `cook.prior_outputs()`,
    // rather than resolved here to a parse-time local. `_cook_outputs_N` exists
    // only for a `cook` step the parser lowered, so a `test` following a unit
    // declared by `cook.add_unit` — how every module target-maker declares its
    // units — saw no source and fell back to `ingredients` or to nothing. The
    // engine covered for that by walking the unit's DAG predecessors, which is
    // exactly what CS-0186 removes. Asking at register phase sees every unit
    // however it was registered, and keeps the answer in the declaration.
    let src = format!("_test_src{line}");
    let source_expr = src.clone();
    out.push_str(&format!("    local {src} = cook.prior_outputs()\n"));
    if has_ingredients {
        out.push_str(&format!(
            "    if #{src} == 0 then {src} = ingredients end\n"
        ));
    }
    // `last_cook_index` no longer selects the source; it survives only as the
    // "is there a preceding producing step at parse time" signal the
    // empty-source guard above uses.
    let _ = last_cook_index;

    // CS-0186 §17.4 rule 1: the iteration source is resolved into the unit's
    // DECLARED inputs, here, at the point the unit is recorded. The engine no
    // longer reconstructs it by walking the unit's predecessors — it could only
    // do that for units it knew to be tests, which made the step kind a fact
    // the cache had to consult, and left the engine holding an input set the
    // declaration did not contain.
    //
    // The mode decides the shape, and the mode is the sigil deduction: a body
    // referencing the iteration item reads ONE item, so that is what its unit
    // declares. What the source happens to BE — resolved ingredients, or the
    // preceding cook step's outputs — decides only what `_test_in` holds.
    //
    // CS-0182 made the one-to-one narrowing conditional on the source being
    // `ingredients`, on the ground that an upstream output declared as an input
    // would be a reclassification. CS-0186 withdraws that: in one-to-one mode
    // the item IS what the unit reads (`test { ./$<in> }` runs one binary), so
    // declaring it is the declaration being accurate. The exclusion cost §8.6's
    // own Example 8.6.1 any reuse at all — both units of `ingredients "src/*.c"`
    // → `cook "build/$<in.stem>"` → `test { ./$<in> }` recorded all four paths,
    // so editing one source re-ran the whole fan-out.
    //
    // Declaring the item rather than the ingredient behind it is also what
    // restores early cutoff: the unit for `build/a` declares `build/a`, so
    // editing `src/a.c` re-runs it only if the rebuilt bytes actually moved.
    let inputs_field: String = match mode {
        // One item per unit, whatever the source is.
        PlateTestMode::OneToOne => "inputs = {_test_in}, ".to_string(),
        // The unit reads the whole source, so the whole source is its input
        // set. Emitted unconditionally: whether a source EXISTS is a
        // register-phase fact, not a parse-time one, and `source_expr` is
        // §8.6.1's fallback chain evaluated there. A recipe that supplies
        // nothing yields an empty list, which leaves the unit with nothing to
        // key on and therefore no key — `test { cargo test }`, running every
        // invocation, which is the rule rather than an accident of codegen.
        _ => format!("inputs = {source_expr}, "),
    };
    let inputs_field: &str = &inputs_field;

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

    match &test_step.body {
        Body::ShellBlock(lines) => match mode {
            PlateTestMode::OneToOne => {
                let cmd_text = cook_contracts::shell_block::compose(lines);
                let mut consulted = ConsultedEnv::new();
                let (cmd_expr, probe_keys) = expand_plate_test_body(
                    &cmd_text, recipe_names, "_test_in", &mut consulted,
                )
                .map_err(|source| CodegenError::SigilResolve { line, source })?;
                reject_probe_refs_in_command(line, probe_keys)?;
                out.push_str(&format!(
                    "    for _, _test_in in ipairs({}) do\n        cook.add_unit({{step_kind = \"test\", command = {}, {}{}line = {}, member = _test_in, consulted_env_keys = {}}})\n    end\n",
                    source_expr, cmd_expr, inputs_field, seal_field, line, consulted.to_lua_table()
                ));
            }
            // `ManyToOne` is grouped in here for exhaustiveness only:
            // `detect_plate_test_mode` never returns it for `Body::ShellBlock`
            // (CS-0130 removed `$<all>`, so a batched test is
            // Lua-block-only), and this arm is otherwise byte-for-byte what
            // the old OneShot arm did.
            PlateTestMode::OneShot | PlateTestMode::ManyToOne => {
                let cmd_text = cook_contracts::shell_block::compose(lines);
                let mut consulted = ConsultedEnv::new();
                let (cmd_expr, probe_keys) = expand_plate_test_body(
                    &cmd_text, recipe_names, "\"\"", &mut consulted,
                )
                .map_err(|source| CodegenError::SigilResolve { line, source })?;
                reject_probe_refs_in_command(line, probe_keys)?;
                out.push_str(&format!(
                    "    cook.add_unit({{step_kind = \"test\", command = {}, {}{}line = {}, consulted_env_keys = {}}})\n",
                    cmd_expr, inputs_field, seal_field, line, consulted.to_lua_table()
                ));
            }
        },
        Body::LuaBlock(code) => match mode {
            PlateTestMode::OneToOne => {
                out.push_str(&format!(
                    "    for _, _test_in in ipairs({}) do\n",
                    source_expr
                ));
                // Binding convention: build lua_code at register time with `local input = <value>`.
                out.push_str(&format!(
                    "        cook.add_unit({{step_kind = \"test\", lua_code = (\"local input = \" .. string.format(\"%q\", _test_in) .. \"\\n\") .. {}, {}{}line = {}, member = _test_in, consulted_env_keys = \"*\"}})\n",
                    lua_chunk_literal(code), inputs_field, seal_field, line
                ));
                out.push_str("    end\n");
            }
            PlateTestMode::ManyToOne => {
                // Serialise the inputs table at register time into the lua_code string.
                out.push_str(&format!(
                    "    cook.add_unit({{step_kind = \"test\", lua_code = (function()\n        local _h = {{\"local inputs = {{\"}}\n        for _i, _v in ipairs({}) do if _i > 1 then _h[#_h+1] = \", \" end _h[#_h+1] = string.format(\"%q\", _v) end\n        _h[#_h+1] = \"}}\\n\"\n        return table.concat(_h) .. {}\n    end)(), {}{}line = {}, consulted_env_keys = \"*\"}})\n",
                    source_expr, lua_chunk_literal(code), inputs_field, seal_field, line
                ));
            }
            PlateTestMode::OneShot => {
                out.push_str(&format!(
                    "    cook.add_unit({{step_kind = \"test\", lua_code = {}, {}{}line = {}, consulted_env_keys = \"*\"}})\n",
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

/// The per-member iteration source, emitted inside the member loop (CS-0186).
///
/// A member unit reads the artifact its own member's producer wrote, so that is
/// what it declares. Both the alternatives are defects the branch fixed
/// elsewhere and would have reintroduced here: declaring the whole fan-out's
/// outputs costs per-member reuse (§17.1 observable 5), and declaring nothing —
/// which is what this path did — leaves a unit the member rule has just made
/// cacheable keyed on nothing its producer wrote, so it replays a pass over an
/// artifact that has since gone bad.
const MEMBER_SOURCE_LINE: &str =
    "        local _test_src = cook.prior_outputs(cook.member_to_string(item))\n";

/// COOK-63 §8.2: lower a `test` step inside a member-fanout recipe to one test
/// unit per data member, with the member bound as `item`. The recipe body has
/// already emitted `local _items = <source>`.
pub(crate) fn generate_member_fanout_test_step(
    out: &mut String,
    test_step: &TestStep,
    line: usize,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    use crate::resolver::{IterMode, OutputShape};
    use crate::template::{cook_step_ctx, expand_member_fanout_template};

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

    match &test_step.body {
        Body::ShellBlock(lines) => {
            let combined = cook_contracts::shell_block::compose(lines);
            let mut consulted = ConsultedEnv::new();
            let (cmd_concat, probe_keys) = expand_member_fanout_template(
                &combined,
                &ctx,
                &mut consulted,
                crate::template::ProbeLowering::CacheGet,
            )
            .map_err(|source| CodegenError::SigilResolve { line, source })?;
            reject_probe_refs_in_command(line, probe_keys)?;
            let cmd_expr = cmd_concat;
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(MEMBER_SOURCE_LINE);
            out.push_str(&format!(
                "        cook.add_unit({{step_kind = \"test\", command = {}, inputs = _test_src, {}line = {}, consulted_env_keys = {}, member = cook.member_to_string(item)}})\n",
                cmd_expr, seal_field, line, consulted.to_lua_table()
            ));
            out.push_str("    end\n");
        }
        Body::LuaBlock(code) => {
            // §8.2: the Lua body sees the member as `item`; execute-phase
            // binding of `item` is wired by the COOK-64 runtime slice.
            out.push_str("    for _, item in ipairs(_items) do\n");
            out.push_str(MEMBER_SOURCE_LINE);
            out.push_str(&format!(
                "        cook.add_unit({{step_kind = \"test\", lua_code = {}, inputs = _test_src, {}line = {}, consulted_env_keys = \"*\", member = cook.member_to_string(item)}})\n",
                lua_chunk_literal(code), seal_field, line
            ));
            out.push_str("    end\n");
        }
    }
    Ok(())
}

/// The single refusal site for a probe-value reference in a `test` shell
/// command, shared by the plain and fan-out test paths (COOK-357).
fn reject_probe_refs_in_command(
    line: usize,
    probe_keys: BTreeSet<String>,
) -> Result<(), CodegenError> {
    if probe_keys.is_empty() {
        return Ok(());
    }
    // BTreeSet already yields keys in sorted order.
    Err(CodegenError::ProbeRefInTestCommand {
        line,
        keys: probe_keys.into_iter().collect(),
    })
}

