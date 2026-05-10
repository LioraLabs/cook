use std::collections::BTreeSet;

use cook_contracts::ACCESSORS;
use cook_lang::ast::*;

use crate::cook_step::generate_cook_step;
use crate::dep_ref::{extract_dep_refs, extract_sigil_tokens};
use crate::lua_string::{escape_lua_string, wrap_lua_string};
use crate::plate_step::generate_plate_step;
use crate::resolver::{IterMode, OutputShape, ResolveCtx};
use crate::sigil;
use crate::template::ConsultedEnv;
use crate::test_step;

/// Error raised by `generate_with_names_checked` when codegen-phase
/// validation rejects a Cookfile.
///
/// Per Cook Standard § 5.4, `$<lib.ACCESSOR>` is only valid in a step whose
/// output pattern declares `lib` as an iteration driver. Appearing in a
/// using-string, plate command, test command, or bare shell without a
/// matching driver is an error.
///
/// Per CS-0033 §6.7, placeholder rules also cover:
/// - bare `$<stem>` / `$<name>` / `$<ext>` / `$<dir>` in output patterns (rejected)
/// - `$<out_N>` in single-output steps (rejected)
/// - `$<out>` in multi-output steps (rejected)
/// - `$<lib.ACCESSOR>` inside using-clause body (rejected)
#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error(
        "line {line}: recipe '{referrer}': '{referent}.{accessor}' appears in {surface} but \
         '{referent}' is not named as an iteration driver in this step's output pattern"
    )]
    AccessorWithoutDriver {
        referrer: String,
        referent: String,
        accessor: String,
        surface: &'static str,
        line: usize,
    },
    /// CS-0022 placeholder validation failure (body or output pattern).
    #[error("line {line}: recipe '{recipe}': {message}")]
    PlaceholderViolation {
        recipe: String,
        message: String,
        line: usize,
    },
    /// CS-0024 plate/test mode or placeholder validation failure.
    #[error("{0}")]
    PlateTest(#[from] crate::plate_step::CodegenError),
}

pub fn generate(cookfile: &Cookfile) -> String {
    generate_with_names(cookfile, &BTreeSet::new())
        .expect("generate: unexpected codegen error (use generate_with_names_checked for validated codegen)")
}

/// Lower `cookfile` and return any register-time warnings alongside the Lua
/// source.
///
/// Current warnings:
/// - **Empty-output reference (Cook Standard § 5.5).** When a `{NAME}` or
///   `{NAME.ACCESSOR}` reference names a recipe whose output list at register
///   time is empty (i.e. has no cook steps and no ingredients), we emit a
///   warning naming both the referrer and referent. The reference itself is
///   still lowered — the substitution is the empty string — so callers must
///   not treat this as an error.
pub fn generate_with_names_and_warnings(
    cookfile: &Cookfile,
    recipe_names: &BTreeSet<String>,
) -> (String, Vec<String>) {
    let warnings = warn_empty_output_refs(cookfile, recipe_names);
    let output = generate_with_names(cookfile, recipe_names)
        .expect("generate_with_names_and_warnings: unexpected codegen error");
    (output, warnings)
}

/// Lower `cookfile` after running codegen-phase validation. Returns an error
/// if any `{NAME.ACCESSOR}` placeholder appears where no output pattern in the
/// same step declares `NAME` as an iteration driver (Cook Standard § 5.4).
pub fn generate_with_names_checked(
    cookfile: &Cookfile,
    recipe_names: &BTreeSet<String>,
) -> Result<String, CodegenError> {
    validate_accessor_placement(cookfile, recipe_names)?;
    generate_with_names(cookfile, recipe_names)
}

/// Detect references whose referent has an empty output list and return one
/// warning per offending (referrer, referent) pair.
///
/// "Empty output list" is approximated at codegen time as: no cook steps AND
/// no ingredients. A recipe with only ingredients still has a non-empty output
/// list per § 5.4.1 passthrough; a recipe whose ingredient globs resolve to
/// nothing at register time is still flagged by the runtime, not here.
fn warn_empty_output_refs(
    cookfile: &Cookfile,
    recipe_names: &BTreeSet<String>,
) -> Vec<String> {
    let empty: BTreeSet<String> = cookfile
        .recipes
        .iter()
        .filter(|r| {
            r.ingredients.is_empty()
                && !r.steps.iter().any(|s| matches!(s, Step::Cook { .. }))
        })
        .map(|r| r.name.clone())
        .collect();

    let mut warnings = Vec::new();
    for recipe in &cookfile.recipes {
        for dep_ref in extract_dep_refs(recipe, recipe_names) {
            if empty.contains(&dep_ref.recipe_name) {
                warnings.push(format!(
                    "recipe '{}' references '{}' which has empty output",
                    recipe.name, dep_ref.recipe_name
                ));
            }
        }
    }
    warnings
}

/// Check that all output patterns of a multi-output cook step share the same
/// iteration driver (all literal, all {in.X}, or all {recipe.X} for the same recipe).
fn check_multi_output_coherence(
    step: &CookStep,
    recipe_names: &BTreeSet<String>,
) -> Result<(), String> {
    if step.outputs.len() < 2 {
        return Ok(());
    }

    use crate::template::output_pattern_kind_with_recipes;
    let first = output_pattern_kind_with_recipes(&step.outputs[0], recipe_names);
    for (idx, out) in step.outputs.iter().enumerate().skip(1) {
        let kind = output_pattern_kind_with_recipes(out, recipe_names);
        if !drivers_match(&first, &kind) {
            return Err(format!(
                "CS-0022: cook step's output #1 ({:?}) and output #{} ({:?}) declare \
                 different iteration drivers; all output patterns must share a driver",
                step.outputs[0],
                idx + 1,
                out
            ));
        }
    }
    Ok(())
}

fn drivers_match(
    a: &crate::template::OutputPatternKind,
    b: &crate::template::OutputPatternKind,
) -> bool {
    use crate::template::OutputPatternKind::*;
    match (a, b) {
        (Literal, Literal) => true,
        (OwnInputAccessor, OwnInputAccessor) => true,
        (DepDriven { dep_name: n1, .. }, DepDriven { dep_name: n2, .. }) => n1 == n2,
        _ => false,
    }
}

/// Validate placeholder forms inside a `cook` step's output pattern.
///
/// Rejects:
/// - Bare path accessors (`$<stem>`, `$<name>`, `$<ext>`, `$<dir>`) — these
///   were removed per CS-0033 §6.7. Canonical form: `$<in.stem>` etc.
/// - Bare recipe references (`$<lib>` with no accessor) where `lib` names an
///   in-scope recipe — Standard §5.4 third bullet: "`$<lib>` has no
///   iteration semantics" inside an output pattern. The accessor form
///   (`$<lib.stem>` etc.) is what enables dep-driven iteration.
fn check_output_pattern_no_bare_accessors(
    pattern: &str,
    recipe: &str,
    line: usize,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    for span in sigil::scan(pattern) {
        let inner = span.ident.as_str();
        match inner {
            "stem" | "name" | "ext" | "dir" => {
                return Err(CodegenError::PlaceholderViolation {
                    recipe: recipe.to_string(),
                    message: format!(
                        "CS-0022: bare $<{inner}> in output pattern was removed; \
                         use $<in.{inner}> (or $<dep.{inner}> for a dep-driven pattern)"
                    ),
                    line,
                });
            }
            _ => {}
        }

        // Standard §5.4: bare `$<lib>` (no accessor) referring to an
        // in-scope recipe is rejected in output patterns. There is no
        // iteration semantics for it here — use `$<lib.stem>` (or another
        // path accessor) to drive iteration over `lib`'s outputs, or move
        // the reference into the `using` body for space-joined substitution.
        if !inner.contains('.') && recipe_names.contains(inner) {
            return Err(CodegenError::PlaceholderViolation {
                recipe: recipe.to_string(),
                message: format!(
                    "$<{inner}> (bare recipe reference) is not allowed in an output \
                     pattern; use $<{inner}.stem> (or another path accessor) for \
                     dep-driven iteration, or move the reference into the `using` body"
                ),
                line,
            });
        }
    }
    Ok(())
}

/// For each cook step, verify that every `{NAME.ACCESSOR}` placeholder in the
/// using-block shares a driver with the output pattern. Reject any accessor
/// placeholder that appears in plate / test / bare shell steps, which have no
/// output pattern and thus cannot declare a driver.
///
/// Also validates CS-0022 §6.7 placeholder rules inside shell-block bodies
/// (replaces the former `panic!` in `generate_cook_step`).
fn validate_accessor_placement(
    cookfile: &Cookfile,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    for recipe in &cookfile.recipes {
        for step in &recipe.steps {
            match step {
                Step::Cook { step: cook_step, line } => {
                    // Check output patterns for bare path accessors (CS-0022 §6.7)
                    // and bare recipe references (Standard §5.4).
                    for pattern in &cook_step.outputs {
                        check_output_pattern_no_bare_accessors(
                            pattern,
                            &recipe.name,
                            *line,
                            recipe_names,
                        )?;
                    }

                    // Multi-output coherence check.
                    if let Err(msg) = check_multi_output_coherence(cook_step, recipe_names) {
                        return Err(CodegenError::PlaceholderViolation {
                            recipe: recipe.name.clone(),
                            message: msg,
                            line: *line,
                        });
                    }

                    let drivers = collect_drivers(&cook_step.outputs, recipe_names);
                    // Check ShellBlock lines for accessor-without-driver and
                    // CS-0022 placeholder rules.
                    if let Some(UsingClause::ShellBlock(lines)) = &cook_step.using_clause {
                        // Determine the mode so validate_placeholders can check
                        // {in}, {out}, {out_N}, {all}, bare accessors, and lib refs.
                        let mode = crate::cook_step::cook_step_mode_with_names(
                            cook_step, recipe_names,
                        );
                        let ctx = crate::template::PlaceholderValidationContext {
                            mode: &mode,
                            declared_output_count: cook_step.outputs.len(),
                            recipe_names,
                        };
                        for shell_line in lines {
                            if let Err(msg) =
                                crate::template::validate_placeholders(shell_line, &ctx)
                            {
                                return Err(CodegenError::PlaceholderViolation {
                                    recipe: recipe.name.clone(),
                                    message: msg,
                                    line: *line,
                                });
                            }
                            check_command(
                                shell_line,
                                &drivers,
                                recipe_names,
                                &recipe.name,
                                "shell-block",
                                *line,
                            )?;
                        }
                    }
                }
                Step::InlineLua { .. } | Step::InlineLuaBlock { .. } => {
                    // Inline Lua bodies are opaque to the accessor-placement
                    // check; the templater does not run on Lua source.
                }
                Step::Plate { step: plate_step, line } => {
                    if let UsingClause::ShellBlock(lines) = &plate_step.body {
                        for shell_line in lines {
                            check_command(
                                shell_line,
                                &BTreeSet::new(),
                                recipe_names,
                                &recipe.name,
                                "plate command",
                                *line,
                            )?;
                        }
                    }
                }
                Step::Test { step: test_step, line } => {
                    if let UsingClause::ShellBlock(lines) = &test_step.body {
                        for shell_line in lines {
                            check_command(
                                shell_line,
                                &BTreeSet::new(),
                                recipe_names,
                                &recipe.name,
                                "test command",
                                *line,
                            )?;
                        }
                    }
                }
                Step::Shell { command, line, .. } => {
                    check_command(
                        command,
                        &BTreeSet::new(),
                        recipe_names,
                        &recipe.name,
                        "shell step",
                        *line,
                    )?;
                }
                Step::Lua { .. } | Step::LuaBlock { .. } => {
                    // Execute-phase Lua bodies are opaque to the templater
                    // accessor-placement check; the {NAME.ACCESSOR} surface
                    // does not run inside a Lua chunk.
                }
                // `Step` is `#[non_exhaustive]`; future step kinds added by the
                // reference implementation pass through this validation pass
                // without surface checks until the codegen learns about them.
                _ => {}
            }
        }
    }
    Ok(())
}

/// Returns true for step kinds that compose into a body unit
/// (§{recipes.body-bundling}). Interactive shell steps and any
/// declarative-region step return false.
fn is_bundleable(step: &Step) -> bool {
    matches!(
        step,
        Step::Shell { interactive: false, .. } | Step::Lua { .. } | Step::LuaBlock { .. }
    )
}

/// Emit one execute-phase body unit for a bundle of imperative-region
/// steps. The bundle is assumed to contain only bundleable steps
/// (§{recipes.body-bundling}). Adjacent non-interactive shell lines
/// coalesce into a single `cook.sh` call inside the unit's `lua_code`
/// payload, with `set -e` prepended so per-line halt-on-failure matches
/// the historical per-shell-unit semantic.
///
/// Shell commands undergo sigil substitution (CS-0033): any `$<IDENT>`
/// placeholder in a command is expanded at codegen time. Commands with no
/// sigil placeholders are coalesced into a raw shell-text `cook.sh` call;
/// commands with sigil placeholders are emitted as Lua expression `cook.sh`
/// calls with the resolved values substituted in.
///
/// The chunk is prefixed with `local <alias> = cook.load_module("<name>")`
/// per `use` declaration in the source Cookfile (CS-0017,
/// §{lua.cook-load-module}).
fn emit_body_unit_with_names(
    out: &mut String,
    bundle: &[Step],
    uses: &[UseStatement],
    recipe_names: &BTreeSet<String>,
) {
    let mut chunk = String::new();
    // Raw shell lines (no sigils) coalesced for cook.sh(long-string).
    let mut shell_run: Vec<String> = Vec::new();

    for use_stmt in uses {
        let lua_name = use_stmt.module_name.replace('-', "_");
        chunk.push_str(&format!(
            "local {} = cook.load_module(\"{}\")\n",
            lua_name,
            escape_lua_string(&use_stmt.module_name),
        ));
    }

    fn flush_raw(chunk: &mut String, run: &mut Vec<String>) {
        if run.is_empty() {
            return;
        }
        let mut joined = String::from("set -e\n");
        for (idx, line) in run.iter().enumerate() {
            if idx > 0 {
                joined.push('\n');
            }
            joined.push_str(line);
        }
        let wrapped = wrap_lua_string(&joined);
        chunk.push_str(&format!("io.write(cook.sh({}))\n", wrapped));
        run.clear();
    }

    fn flush_sigil_cmd(chunk: &mut String, lua_expr: &str) {
        // A sigil-expanded command: emit as cook.sh(lua_expr) inline.
        // Prepend "set -e\n" so fail-fast semantics hold.
        let sh_arg = format!("\"set -e\\n\" .. {}", lua_expr);
        chunk.push_str(&format!("io.write(cook.sh({}))\n", sh_arg));
    }

    for step in bundle {
        match step {
            Step::Shell { command, interactive: false, .. } => {
                let has_sigils = !crate::sigil::scan(command).is_empty();
                if has_sigils {
                    // Flush any accumulated raw lines before this sigil command.
                    flush_raw(&mut chunk, &mut shell_run);
                    // Expand sigil template and emit as a Lua expression.
                    let ctx = ResolveCtx {
                        mode: IterMode::OneShot,
                        outputs: OutputShape::None,
                        recipes_in_scope: recipe_names,
                    };
                    let mut consulted = ConsultedEnv::new();
                    let lua_expr = match crate::template::expand_sigil_template(command, &ctx, &mut consulted) {
                        Ok(e) => e,
                        Err(e) => format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string())),
                    };
                    flush_sigil_cmd(&mut chunk, &lua_expr);
                } else {
                    // No sigils — accumulate as raw shell text (old behavior).
                    shell_run.push(command.clone());
                }
            }
            Step::Lua { code, .. } => {
                flush_raw(&mut chunk, &mut shell_run);
                chunk.push_str(code);
                if !code.ends_with('\n') {
                    chunk.push('\n');
                }
            }
            Step::LuaBlock { code, .. } => {
                flush_raw(&mut chunk, &mut shell_run);
                chunk.push_str(code);
                if !code.ends_with('\n') {
                    chunk.push('\n');
                }
            }
            _ => unreachable!("emit_body_unit called with non-bundleable step"),
        }
    }
    flush_raw(&mut chunk, &mut shell_run);

    if chunk.is_empty() {
        return;
    }

    let wrapped = wrap_lua_string(&chunk);
    // cache = false: consulted_env_keys is a cache-keying hint, omitted for
    // units that are never cached. The cacheable cook-step path in
    // cook_step.rs is the only emission site that includes it.
    out.push_str(&format!(
        "    cook.add_unit({{lua_code = {}, cache = false}})\n",
        wrapped
    ));
}

fn collect_drivers(
    output_patterns: &[String],
    recipe_names: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut drivers = BTreeSet::new();
    for pat in output_patterns {
        for token in extract_sigil_tokens(pat) {
            if let Some(dot) = token.rfind('.') {
                let prefix = &token[..dot];
                let suffix = &token[dot + 1..];
                if ACCESSORS.contains(&suffix) && recipe_names.contains(prefix) {
                    drivers.insert(prefix.to_string());
                }
            }
        }
    }
    drivers
}

fn check_command(
    command: &str,
    drivers: &BTreeSet<String>,
    recipe_names: &BTreeSet<String>,
    referrer: &str,
    surface: &'static str,
    line: usize,
) -> Result<(), CodegenError> {
    for token in extract_sigil_tokens(command) {
        if let Some(dot) = token.rfind('.') {
            let prefix = &token[..dot];
            let suffix = &token[dot + 1..];
            if ACCESSORS.contains(&suffix)
                && recipe_names.contains(prefix)
                && !drivers.contains(prefix)
            {
                return Err(CodegenError::AccessorWithoutDriver {
                    referrer: referrer.to_string(),
                    referent: prefix.to_string(),
                    accessor: suffix.to_string(),
                    surface,
                    line,
                });
            }
        }
    }
    Ok(())
}

pub fn generate_with_names(
    cookfile: &Cookfile,
    recipe_names: &BTreeSet<String>,
) -> Result<String, CodegenError> {
    let mut out = String::from("-- Generated by Cook\n");

    // Emit module loading for use statements
    for use_stmt in &cookfile.uses {
        let lua_name = use_stmt.module_name.replace('-', "_");
        out.push_str(&format!(
            "local {} = cook.load_module(\"{}\")\n",
            lua_name,
            escape_lua_string(&use_stmt.module_name),
        ));
    }
    if !cookfile.uses.is_empty() {
        out.push('\n');
    }

    // Config-block dispatcher
    if !cookfile.config_blocks.is_empty() {
        out.push_str("function __cook_run_config_blocks(selected_name)\n");

        // Unnamed (base) block — always runs.
        // Comment lines (starting with '#') are skipped; they are Cookfile
        // source comments and are not valid Lua syntax.
        for block in &cookfile.config_blocks {
            if block.name.is_none() {
                for line in block.body.lines() {
                    if line.trim_start().starts_with('#') {
                        continue;
                    }
                    out.push_str("    ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }

        // Named blocks — run if selected_name matches
        let has_named = cookfile.config_blocks.iter().any(|b| b.name.is_some());
        if has_named {
            out.push_str("    if selected_name ~= nil then\n");
            for block in &cookfile.config_blocks {
                if let Some(name) = &block.name {
                    out.push_str(&format!(
                        "        if selected_name == \"{}\" then\n",
                        escape_lua_string(name)
                    ));
                    for line in block.body.lines() {
                        if line.trim_start().starts_with('#') {
                            continue;
                        }
                        out.push_str("            ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push_str("        end\n");
                }
            }
            out.push_str("    end\n");
        }

        out.push_str("end\n\n");
    }

    for recipe in &cookfile.recipes {
        out.push_str(&format!(
            "cook.recipe(\"{}\", {}, function()\n",
            escape_lua_string(&recipe.name),
            generate_metadata(recipe)
        ));

        // Emit local ingredients variable when recipe has ingredients
        if !recipe.ingredients.is_empty() {
            let includes: Vec<String> = recipe
                .ingredients
                .iter()
                .map(|s| format!("\"{}\"", escape_lua_string(s)))
                .collect();
            let excludes: Vec<String> = recipe
                .excludes
                .iter()
                .map(|s| format!("\"{}\"", escape_lua_string(s)))
                .collect();
            out.push_str(&format!(
                "    local ingredients = cook.resolve_ingredients({{{}}}, {{{}}})\n",
                includes.join(", "),
                excludes.join(", "),
            ));
        }

        let mut prev_cook_index: Option<usize> = None;
        let mut cook_index: usize = 0;

        let mut i = 0;
        while i < recipe.steps.len() {
            match &recipe.steps[i] {
                Step::InlineLua { code, .. } => {
                    // §{recipes.lua-steps}: register-phase, inlined into the
                    // recipe-body Lua function.
                    out.push_str(&format!("    {}\n", code));
                    i += 1;
                }
                Step::InlineLuaBlock { code, .. } => {
                    // §{recipes.lua-steps}: register-phase, inlined.
                    for code_line in code.lines() {
                        out.push_str(&format!("    {}\n", code_line));
                    }
                    i += 1;
                }
                Step::Cook {
                    step: cook_step,
                    line,
                } => {
                    cook_index += 1;
                    out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", cook_index));
                    out.push_str("    cook.step_group(function()\n");
                    generate_cook_step(
                        &mut out,
                        cook_step,
                        *line,
                        cook_index,
                        prev_cook_index,
                        &recipe.ingredients,
                        recipe_names,
                    );
                    out.push_str("    end)\n");
                    prev_cook_index = Some(cook_index);
                    i += 1;
                }
                Step::Plate {
                    step: plate_step,
                    line,
                } => {
                    out.push_str("    cook.step_group(function()\n");
                    generate_plate_step(
                        &mut out,
                        plate_step,
                        *line,
                        prev_cook_index,
                        !recipe.ingredients.is_empty(),
                        recipe_names,
                    )?;
                    out.push_str("    end)\n");
                    i += 1;
                }
                Step::Test {
                    step: test_step_val,
                    line,
                } => {
                    out.push_str("    cook.step_group(function()\n");
                    test_step::generate_test_step(
                        &mut out,
                        test_step_val,
                        *line,
                        prev_cook_index,
                        !recipe.ingredients.is_empty(),
                        recipe_names,
                    )?;
                    out.push_str("    end)\n");
                    i += 1;
                }
                Step::Shell { interactive: true, command, line } => {
                    // §{exec.interactive-drain}: own draining unit, breaks
                    // body-bundling (the next imperative step starts a fresh
                    // body unit).
                    // Apply sigil substitution to the command (CS-0033).
                    let cmd_expr = expand_shell_command_sigil(command, recipe_names);
                    // cache = false: consulted_env_keys is a cache-keying hint, omitted for
                    // units that are never cached. The cacheable cook-step path in
                    // cook_step.rs is the only emission site that includes it.
                    out.push_str(&format!(
                        "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
                        cmd_expr, line
                    ));
                    i += 1;
                }
                Step::Shell { interactive: false, .. }
                | Step::Lua { .. }
                | Step::LuaBlock { .. } => {
                    // §{recipes.body-bundling}: coalesce a run of
                    // execute-phase imperative steps into one body unit.
                    let bundle_start = i;
                    while i < recipe.steps.len() && is_bundleable(&recipe.steps[i]) {
                        i += 1;
                    }
                    emit_body_unit_with_names(
                        &mut out,
                        &recipe.steps[bundle_start..i],
                        &cookfile.uses,
                        recipe_names,
                    );
                }
                // `Step` is `#[non_exhaustive]`. Future step kinds added by the
                // reference implementation that this codegen has not yet learned
                // about are skipped — the validator pass above already accepts
                // them silently and runtime never sees them in a generated
                // recipe.
                _ => {
                    i += 1;
                }
            }
        }

        out.push_str("end)\n\n");
    }

    // Emit chores after recipes. Chores compile to the same `cook.recipe(...)`
    // shape as recipes, but every unit has `cache = false` and every shell step
    // has `interactive = true` (§{chores.no-caching}, §{exec.interactive-drain}).
    for chore in &cookfile.chores {
        out.push_str(&compile_chore(chore, &cookfile.uses));
    }

    Ok(out)
}

/// Expand a single shell command through sigil substitution (CS-0033).
/// Returns a Lua expression suitable for the `command =` field of `cook.add_unit`.
/// Commands with no sigil placeholders are emitted as Lua long-string literals.
fn expand_shell_command_sigil(command: &str, recipe_names: &BTreeSet<String>) -> String {
    let has_sigils = !crate::sigil::scan(command).is_empty();
    if !has_sigils {
        return wrap_lua_string(command);
    }
    let ctx = ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: recipe_names,
    };
    let mut consulted = ConsultedEnv::new();
    match crate::template::expand_sigil_template(command, &ctx, &mut consulted) {
        Ok(e) => e,
        Err(e) => format!("\"[[SIGIL_ERROR: {}]]\"", escape_lua_string(&e.to_string())),
    }
}

/// Compile a `Chore` to register-phase Lua.
///
/// A chore compiles to the same shape as a recipe body (a `cook.recipe(...)`
/// call), with two chore-specific differences:
///
/// 1. Every `Step::Shell` is emitted with `interactive = true`.  The parser
///    already enforces this; codegen passes it through.
/// 2. Every `cook.add_unit` records `cache = false`.  No body-bundling
///    across shell steps: because all chore shells are interactive, the
///    existing `is_bundleable` predicate already breaks the bundle at each
///    shell step, so one-shell-per-unit comes for free.
///
/// The generated Lua wraps the body with `cook._enter_chore()` / `cook._exit_chore()`
/// so the runtime can enforce §{chores.no-caching} (see `unit_api.rs`).
pub fn compile_chore(chore: &Chore, uses: &[UseStatement]) -> String {
    let mut out = String::new();

    // Emit chore metadata (only deps, no ingredients/excludes for chores).
    let meta = if chore.deps.is_empty() {
        "{}".to_string()
    } else {
        let items: Vec<String> = chore
            .deps
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        format!("{{requires = {{{}}}}}", items.join(", "))
    };

    out.push_str(&format!(
        "cook.recipe(\"{}\", {}, function()\n",
        escape_lua_string(&chore.name),
        meta,
    ));

    // Mark chore-body start so cook.add_unit can enforce §{chores.no-caching}.
    out.push_str("    cook._enter_chore()\n");

    // Emit steps. All shell steps are interactive (parser guarantees this).
    // Consecutive Lua steps may still coalesce into a body unit, but shell
    // steps always stand alone (interactive = true => not bundleable).
    let mut i = 0;
    while i < chore.steps.len() {
        match &chore.steps[i] {
            Step::Shell { command, line, interactive: true } => {
                // Apply sigil substitution (CS-0033 closes App. E.8).
                let cmd_expr = expand_shell_command_sigil(command, &BTreeSet::new());
                // cache = false: consulted_env_keys is a cache-keying hint, omitted for
                // units that are never cached. The cacheable cook-step path in
                // cook_step.rs is the only emission site that includes it.
                out.push_str(&format!(
                    "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
                    cmd_expr, line
                ));
                i += 1;
            }
            Step::Shell { interactive: false, .. } => {
                // Parser enforces all chore shells are interactive; this arm
                // is unreachable in a well-formed AST, but emit defensively.
                if let Step::Shell { command, line, .. } = &chore.steps[i] {
                    let cmd_expr = expand_shell_command_sigil(command, &BTreeSet::new());
                    // cache = false: consulted_env_keys is a cache-keying hint, omitted for
                    // units that are never cached. The cacheable cook-step path in
                    // cook_step.rs is the only emission site that includes it.
                    out.push_str(&format!(
                        "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
                        cmd_expr, line
                    ));
                }
                i += 1;
            }
            Step::Lua { .. } | Step::LuaBlock { .. } => {
                // Coalesce consecutive execute-phase Lua steps into one body
                // unit (same rule as in recipes), but force cache = false.
                let bundle_start = i;
                while i < chore.steps.len()
                    && matches!(&chore.steps[i], Step::Lua { .. } | Step::LuaBlock { .. })
                {
                    i += 1;
                }
                emit_chore_body_unit(&mut out, &chore.steps[bundle_start..i], uses);
            }
            Step::InlineLua { code, .. } => {
                out.push_str(&format!("    {}\n", code));
                i += 1;
            }
            Step::InlineLuaBlock { code, .. } => {
                for code_line in code.lines() {
                    out.push_str(&format!("    {}\n", code_line));
                }
                i += 1;
            }
            // Cook / Plate / Test steps are banned in chores by the parser.
            _ => {
                i += 1;
            }
        }
    }

    // Mark chore-body end.
    out.push_str("    cook._exit_chore()\n");

    out.push_str("end)\n\n");
    out
}

/// Emit a body unit for a bundle of execute-phase Lua steps within a chore.
///
/// Identical to `emit_body_unit` except the `cook.add_unit` call always
/// passes `cache = false` (chores never cache — §{chores.no-caching}).
fn emit_chore_body_unit(out: &mut String, bundle: &[Step], uses: &[UseStatement]) {
    let mut chunk = String::new();
    let mut shell_run: Vec<String> = Vec::new();

    for use_stmt in uses {
        let lua_name = use_stmt.module_name.replace('-', "_");
        chunk.push_str(&format!(
            "local {} = cook.load_module(\"{}\")\n",
            lua_name,
            escape_lua_string(&use_stmt.module_name),
        ));
    }

    fn flush(chunk: &mut String, run: &mut Vec<String>) {
        if run.is_empty() {
            return;
        }
        let mut joined = String::from("set -e\n");
        for (idx, line) in run.iter().enumerate() {
            if idx > 0 {
                joined.push('\n');
            }
            joined.push_str(line);
        }
        let wrapped = wrap_lua_string(&joined);
        chunk.push_str(&format!("io.write(cook.sh({}))\n", wrapped));
        run.clear();
    }

    for step in bundle {
        match step {
            Step::Lua { code, .. } => {
                flush(&mut chunk, &mut shell_run);
                chunk.push_str(code);
                if !code.ends_with('\n') {
                    chunk.push('\n');
                }
            }
            Step::LuaBlock { code, .. } => {
                flush(&mut chunk, &mut shell_run);
                chunk.push_str(code);
                if !code.ends_with('\n') {
                    chunk.push('\n');
                }
            }
            _ => unreachable!("emit_chore_body_unit called with non-Lua step"),
        }
    }
    flush(&mut chunk, &mut shell_run);

    if chunk.is_empty() {
        return;
    }

    let wrapped = wrap_lua_string(&chunk);
    out.push_str(&format!(
        "    cook.add_unit({{lua_code = {}, interactive = true, cache = false}})\n",
        wrapped
    ));
}

fn generate_metadata(recipe: &Recipe) -> String {
    let mut fields = Vec::new();
    if !recipe.ingredients.is_empty() {
        let items: Vec<String> = recipe
            .ingredients
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("ingredients = {{{}}}", items.join(", ")));
    }
    if !recipe.excludes.is_empty() {
        let items: Vec<String> = recipe
            .excludes
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("excludes = {{{}}}", items.join(", ")));
    }
    if !recipe.deps.is_empty() {
        let items: Vec<String> = recipe
            .deps
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("requires = {{{}}}", items.join(", ")));
    }
    if fields.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", fields.join(", "))
    }
}
