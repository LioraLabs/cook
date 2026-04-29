use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::cook_step::generate_cook_step;
use crate::dep_ref::{extract_brace_tokens, extract_dep_refs};
use crate::lua_string::{escape_lua_string, wrap_lua_string};
use crate::plate_step::generate_plate_step;
use crate::test_step;

/// Known accessor suffixes for `{dep.accessor}` syntax.
/// Keep in lockstep with `dep_ref::ACCESSORS`.
const ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

/// Error raised by `generate_with_names_checked` when codegen-phase
/// validation rejects a Cookfile.
///
/// Per Cook Standard § 5.4, `{lib.ACCESSOR}` is only valid in a step whose
/// output pattern declares `lib` as an iteration driver. Appearing in a
/// using-string, plate command, test command, or bare shell without a
/// matching driver is an error.
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
}

pub fn generate(cookfile: &Cookfile) -> String {
    generate_with_names(cookfile, &BTreeSet::new())
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
    let output = generate_with_names(cookfile, recipe_names);
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
    Ok(generate_with_names(cookfile, recipe_names))
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

/// For each cook step, verify that every `{NAME.ACCESSOR}` placeholder in the
/// using-string shares a driver with the output pattern. Reject any accessor
/// placeholder that appears in plate / test / bare shell steps, which have no
/// output pattern and thus cannot declare a driver.
fn validate_accessor_placement(
    cookfile: &Cookfile,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    for recipe in &cookfile.recipes {
        for step in &recipe.steps {
            match step {
                Step::Cook { step: cook_step, line } => {
                    let drivers = collect_drivers(&cook_step.outputs, recipe_names);
                    if let Some(UsingClause::Shell(cmd)) = &cook_step.using_clause {
                        check_command(
                            cmd,
                            &drivers,
                            recipe_names,
                            &recipe.name,
                            "using-string",
                            *line,
                        )?;
                    }
                }
                Step::InlineLua { .. } | Step::InlineLuaBlock { .. } => {
                    // Inline Lua bodies are opaque to the accessor-placement
                    // check; the templater does not run on Lua source.
                }
                Step::Plate { step: plate_step, line } => {
                    check_command(
                        &plate_step.command,
                        &BTreeSet::new(),
                        recipe_names,
                        &recipe.name,
                        "plate command",
                        *line,
                    )?;
                }
                Step::Test { step: test_step, line } => {
                    check_command(
                        &test_step.command,
                        &BTreeSet::new(),
                        recipe_names,
                        &recipe.name,
                        "test command",
                        *line,
                    )?;
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
/// The chunk is prefixed with `local <alias> = cook.load_module("<name>")`
/// per `use` declaration in the source Cookfile (CS-0017,
/// §{lua.cook-load-module}). This makes the same alias visible to
/// imperative-region Lua as is visible to declarative-region Lua, so
/// `> compile.assemble(target)` reads identically to
/// `compile.plan(target)` without a `cook.` prefix.
fn emit_body_unit(out: &mut String, bundle: &[Step], uses: &[UseStatement]) {
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
        // io.write echoes the captured stdout to the worker's stdout so the
        // user sees `pwd`-style output the way they would in a Make `.ONESHELL`
        // recipe. The cook.sh return value is still discarded; callers who
        // need to consume it should call cook.sh from a `>` line directly.
        chunk.push_str(&format!("io.write(cook.sh({}))\n", wrapped));
        run.clear();
    }

    for step in bundle {
        match step {
            Step::Shell { command, interactive: false, .. } => {
                shell_run.push(command.clone());
            }
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
            _ => unreachable!("emit_body_unit called with non-bundleable step"),
        }
    }
    flush(&mut chunk, &mut shell_run);

    if chunk.is_empty() {
        return;
    }

    let wrapped = wrap_lua_string(&chunk);
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
        for token in extract_brace_tokens(pat) {
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
    for token in extract_brace_tokens(command) {
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

pub fn generate_with_names(cookfile: &Cookfile, recipe_names: &BTreeSet<String>) -> String {
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

        // Unnamed (base) block — always runs
        for block in &cookfile.config_blocks {
            if block.name.is_none() {
                for line in block.body.lines() {
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
                    generate_plate_step(&mut out, plate_step, *line, prev_cook_index, recipe_names);
                    out.push_str("    end)\n");
                    i += 1;
                }
                Step::Test {
                    step: test_step_val,
                    line,
                } => {
                    out.push_str("    cook.step_group(function()\n");
                    test_step::generate_test_step(&mut out, test_step_val, *line, prev_cook_index, recipe_names);
                    out.push_str("    end)\n");
                    i += 1;
                }
                Step::Shell { interactive: true, command, line } => {
                    // §{exec.interactive-drain}: own draining unit, breaks
                    // body-bundling (the next imperative step starts a fresh
                    // body unit).
                    let wrapped = wrap_lua_string(command);
                    out.push_str(&format!(
                        "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
                        wrapped, line
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
                    emit_body_unit(
                        &mut out,
                        &recipe.steps[bundle_start..i],
                        &cookfile.uses,
                    );
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

    out
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
                let wrapped = wrap_lua_string(command);
                out.push_str(&format!(
                    "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
                    wrapped, line
                ));
                i += 1;
            }
            Step::Shell { interactive: false, .. } => {
                // Parser enforces all chore shells are interactive; this arm
                // is unreachable in a well-formed AST, but emit defensively.
                // Treat as interactive to match the chore contract.
                if let Step::Shell { command, line, .. } = &chore.steps[i] {
                    let wrapped = wrap_lua_string(command);
                    out.push_str(&format!(
                        "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
                        wrapped, line
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
        "    cook.add_unit({{lua_code = {}, cache = false}})\n",
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
