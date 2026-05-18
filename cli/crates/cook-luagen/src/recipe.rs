use std::collections::BTreeSet;

use cook_contracts::{ACCESSORS, REGISTER_SURFACE_CHORE_NAME, REGISTER_SURFACE_NAME};
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

/// One contiguous piece of an execute-phase body unit's `lua_code` chunk.
///
/// `Static` pieces are raw Lua source text that the worker VM evaluates
/// directly; they wrap into a long-string literal in the emitted register-phase
/// Lua. `RegisterTimeShellCmd` pieces hold a Lua expression that resolves at
/// register time to a *shell command string* — typically because the original
/// shell command contained a `$<NAME>` recipe ref or a `$<HOME>` env ref that
/// lowered to `cook.dep_output(...)` / `cook.require_env(...)`. Those calls
/// only exist on the register VM, so we evaluate them at register time and
/// bake the resolved string back into the worker's chunk as a literal.
enum ChunkPiece {
    /// A block of static Lua source. Will appear inside a `[[ ... ]]` long
    /// string in the emitted register-phase Lua.
    Static(String),
    /// A Lua expression evaluated at register time that yields a shell command
    /// (e.g. `"echo " .. cook.dep_output("greet")`). The emitted register-phase
    /// Lua wraps this with `"io.write(cook.sh(" .. string.format("%q", <expr>) .. "))\n"`
    /// so the worker only ever sees a literal `io.write(cook.sh("..."))` chunk.
    RegisterTimeShellCmd(String),
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
/// commands with sigil placeholders are split into a `RegisterTimeShellCmd`
/// piece so that any `cook.dep_output(...)` or `cook.require_env(...)` call
/// in the resolved Lua expression evaluates on the register VM (where those
/// helpers are installed). Cook Standard §5.5 requires `$<NAME>` to substitute
/// in any bare `shell_command` body; the worker VM has no `cook.dep_output` /
/// `cook.require_env`, so resolving at register time is the only place these
/// calls can succeed.
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
    let mut pieces: Vec<ChunkPiece> = Vec::new();
    // Raw shell lines (no sigils) coalesced for cook.sh(long-string).
    let mut shell_run: Vec<String> = Vec::new();
    // Buffer of static Lua source text (use-stmts, raw-shell flushes,
    // Lua-step bodies). Flushed into `pieces` whenever we hit a
    // RegisterTimeShellCmd boundary.
    let mut static_buf = String::new();

    for use_stmt in uses {
        let lua_name = use_stmt.module_name.replace('-', "_");
        static_buf.push_str(&format!(
            "local {} = cook.load_module(\"{}\")\n",
            lua_name,
            escape_lua_string(&use_stmt.module_name),
        ));
    }

    fn flush_raw_into_static(static_buf: &mut String, run: &mut Vec<String>) {
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
        static_buf.push_str(&format!("io.write(cook.sh({}))\n", wrapped));
        run.clear();
    }

    fn flush_static(pieces: &mut Vec<ChunkPiece>, static_buf: &mut String) {
        if !static_buf.is_empty() {
            pieces.push(ChunkPiece::Static(std::mem::take(static_buf)));
        }
    }

    for step in bundle {
        match step {
            Step::Shell { command, interactive: false, .. } => {
                let has_sigils = !crate::sigil::scan(command).is_empty();
                if has_sigils {
                    // Flush any accumulated raw lines (into static_buf) so they
                    // run before this sigil command.
                    flush_raw_into_static(&mut static_buf, &mut shell_run);
                    // Expand sigil template; the result is a Lua expression that
                    // may reference `cook.dep_output(...)` / `cook.require_env(...)`
                    // — both register-VM-only. Ship it as a RegisterTimeShellCmd
                    // piece so it evaluates on the right VM.
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
                    // Prepend "set -e\n" so per-line halt-on-failure semantics
                    // match raw-shell flushes.
                    let with_set_e = format!("\"set -e\\n\" .. ({})", lua_expr);
                    flush_static(&mut pieces, &mut static_buf);
                    pieces.push(ChunkPiece::RegisterTimeShellCmd(with_set_e));
                } else {
                    // No sigils — accumulate as raw shell text (old behavior).
                    shell_run.push(command.clone());
                }
            }
            Step::Lua { code, .. } => {
                flush_raw_into_static(&mut static_buf, &mut shell_run);
                static_buf.push_str(code);
                if !code.ends_with('\n') {
                    static_buf.push('\n');
                }
            }
            Step::LuaBlock { code, .. } => {
                flush_raw_into_static(&mut static_buf, &mut shell_run);
                static_buf.push_str(code);
                if !code.ends_with('\n') {
                    static_buf.push('\n');
                }
            }
            _ => unreachable!("emit_body_unit called with non-bundleable step"),
        }
    }
    flush_raw_into_static(&mut static_buf, &mut shell_run);
    flush_static(&mut pieces, &mut static_buf);

    if pieces.is_empty() {
        return;
    }

    let lua_code_expr = render_chunk_pieces(&pieces);
    // cache = false: consulted_env_keys is a cache-keying hint, omitted for
    // units that are never cached. The cacheable cook-step path in
    // cook_step.rs is the only emission site that includes it.
    out.push_str(&format!(
        "    cook.add_unit({{lua_code = {}, cache = false}})\n",
        lua_code_expr
    ));
}

/// Render a sequence of `ChunkPiece`s into a single Lua expression suitable
/// for use as the `lua_code = ...` value in an `cook.add_unit` call.
///
/// All-Static sequences emit as a single long-string literal (preserving the
/// pre-fix output shape for the common case). Mixed sequences emit as a
/// concatenation: each `Static` piece becomes a long-string literal, each
/// `RegisterTimeShellCmd(expr)` becomes
/// `"io.write(cook.sh(" .. string.format("%q", expr) .. "))\n"`. The worker
/// VM therefore receives a chunk where every `cook.sh(...)` call has a
/// pre-resolved string literal as its argument.
fn render_chunk_pieces(pieces: &[ChunkPiece]) -> String {
    let all_static = pieces.iter().all(|p| matches!(p, ChunkPiece::Static(_)));
    if all_static {
        // Single concatenated static buffer — keep the pre-fix long-string
        // shape so existing snapshots / conformance fixtures stay byte-stable.
        let mut buf = String::new();
        for p in pieces {
            if let ChunkPiece::Static(s) = p {
                buf.push_str(s);
            }
        }
        return wrap_lua_string(&buf);
    }
    let mut parts: Vec<String> = Vec::new();
    for p in pieces {
        match p {
            ChunkPiece::Static(s) if s.is_empty() => {}
            ChunkPiece::Static(s) => {
                parts.push(wrap_lua_string(s));
            }
            ChunkPiece::RegisterTimeShellCmd(expr) => {
                // Wrap the resolved shell command into an `io.write(cook.sh("..."))`
                // line. `string.format("%q", s)` returns a Lua-quoted literal
                // (handles embedded quotes / backslashes / newlines), so the
                // result is a safe drop-in inside the `cook.sh(...)` call.
                parts.push(format!(
                    "\"io.write(cook.sh(\" .. string.format(\"%q\", {}) .. \"))\\n\"",
                    expr
                ));
            }
        }
    }
    parts.join(" .. ")
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

/// A top-level emission target. Sorted by source line so we emit in
/// source order, interleaving recipes, chores, register blocks, and
/// top-level module_calls per Standard §3.7 / §3.7.5 splicing semantics.
enum TopLevelItem<'a> {
    Recipe(&'a Recipe),
    Chore(&'a Chore),
    RegisterBlock(&'a RegisterBlock),
    TopLevelModuleCall(&'a TopLevelModuleCall),
}

impl<'a> TopLevelItem<'a> {
    fn line(&self) -> usize {
        match self {
            TopLevelItem::Recipe(r)             => r.line,
            TopLevelItem::Chore(c)              => c.line,
            TopLevelItem::RegisterBlock(rb)     => rb.line,
            TopLevelItem::TopLevelModuleCall(c) => c.line,
        }
    }
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

    // Source-ordered merge of recipes, chores, register blocks, and
    // top-level module_calls. Register-block bodies and top-level
    // module_call source splice verbatim into the top-level chunk per
    // Standard §3.7 / §3.7.5; recipes and chores emit as cook.recipe(...)
    // calls. Source order matters: items that depend on earlier register-
    // block / module-call locals must appear AFTER those items.
    let mut items: Vec<TopLevelItem> = cookfile
        .recipes
        .iter()
        .map(TopLevelItem::Recipe)
        .chain(cookfile.chores.iter().map(TopLevelItem::Chore))
        .chain(cookfile.register_blocks.iter().map(TopLevelItem::RegisterBlock))
        .chain(cookfile.top_level_module_calls.iter().map(TopLevelItem::TopLevelModuleCall))
        .collect();
    items.sort_by_key(|i| i.line());

    for item in items {
        match item {
            TopLevelItem::RegisterBlock(rb) => {
                // Splice the body verbatim into the top-level chunk.
                // Comment lines (Cookfile syntax, leading `#`) are skipped;
                // blank lines and Lua content are preserved as-is.
                for line in rb.body.lines() {
                    if line.trim_start().starts_with('#') {
                        continue;
                    }
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            }
            TopLevelItem::TopLevelModuleCall(call) => {
                // Splice the collected call source verbatim. Same shape as
                // a register_block containing only that call.
                for line in call.code.lines() {
                    if line.trim_start().starts_with('#') {
                        continue;
                    }
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            }
            TopLevelItem::Recipe(recipe) => {
                // SHI-222 Phase 3 Task 3.1: surface `recipe NAME` blocks
                // lower to the codegen-private `cook.__register_surface`
                // helper (with `__line = N`) so collision detection can
                // distinguish surface declarations from dynamic
                // `cook.recipe(...)` calls. The public `cook.recipe` API
                // is unchanged; this is a Standard-internal split (CS-0077).
                out.push_str(&format!(
                    "cook.{}(\"{}\", {}, function()\n",
                    REGISTER_SURFACE_NAME,
                    escape_lua_string(&recipe.name),
                    generate_metadata_with_line(recipe, recipe_names)
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
            TopLevelItem::Chore(chore) => {
                out.push_str(&compile_chore(chore, &cookfile.uses));
            }
        }
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

    // SHI-222 Phase 3 Task 3.1: surface `chore NAME` blocks lower to the
    // codegen-private `cook.__register_surface_chore` helper (with
    // `__line = N`). The register-phase capture closure tags the
    // registration with `RecipeKind::Chore` so collision detection and
    // CLI dispatch can distinguish chores from recipes. Chores have no
    // ingredients/excludes (parser-enforced), only `requires`.
    let mut fields = chore_metadata_fields(chore);
    fields.push(format!("__line = {}", chore.line));
    let meta = format!("{{{}}}", fields.join(", "));

    out.push_str(&format!(
        "cook.{}(\"{}\", {}, function()\n",
        REGISTER_SURFACE_CHORE_NAME,
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

/// Render a `Recipe`'s register-phase metadata table with an explicit
/// `__line = N` field. Used by the surface codegen path
/// (`cook.__register_surface(...)`) so the register-phase capture closure
/// can tag the registration with the source line — `caller_line_in_cookfile`'s
/// call-stack walk does not work here because the codegen splices into the
/// top-level chunk and the call site line is the *generated* line, not the
/// original Cookfile line. The `__line` field is always present so the
/// emitted table is non-empty even for a recipe with no `requires` /
/// `ingredients` / `excludes`.
fn generate_metadata_with_line(recipe: &Recipe, recipe_names: &BTreeSet<String>) -> String {
    let mut fields = recipe_metadata_fields(recipe, recipe_names);
    fields.push(format!("__line = {}", recipe.line));
    format!("{{{}}}", fields.join(", "))
}

/// Field-builder for `generate_metadata_with_line`. Emits one
/// `KEY = {...}` entry per non-empty list metadata field, in the historical
/// order (`ingredients`, `excludes`, `requires`). The `__line = N` field is
/// appended by the caller.
///
/// `requires` merges the explicit `requires`/`deps` declaration with cross-
/// recipe `$<NAME>` body references found by `dep_ref::extract_dep_refs`. In
/// the unified register-phase model (CS-0077), recipe bodies run in topo
/// order during the register pass; a body that calls `cook.dep_output(NAME)`
/// (the codegen lowering for `$<NAME>`) must have its target already
/// registered when it runs. Inferring requires from body refs keeps the
/// pre-unified behaviour where AST-walked inferred deps drove wave ordering.
fn recipe_metadata_fields(recipe: &Recipe, recipe_names: &BTreeSet<String>) -> Vec<String> {
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

    // Build the unified requires set: explicit `recipe.deps` ∪ inferred
    // cross-recipe refs from `$<NAME>` body tokens. Preserves declared order
    // (explicit first, then inferred in alphabetical order via BTreeSet).
    let mut requires: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for d in &recipe.deps {
        if seen.insert(d.clone()) {
            requires.push(d.clone());
        }
    }
    let inferred = crate::dep_ref::extract_dep_refs(recipe, recipe_names);
    for r in inferred {
        if seen.insert(r.recipe_name.clone()) {
            requires.push(r.recipe_name);
        }
    }
    if !requires.is_empty() {
        let items: Vec<String> = requires
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("requires = {{{}}}", items.join(", ")));
    }
    fields
}

/// Build chore metadata fields. Chores have no ingredients/excludes,
/// only `requires` (chore `deps`). Used by `compile_chore` to assemble
/// the surface-shape (with `__line`) metadata table for
/// `cook.__register_surface_chore(...)`.
fn chore_metadata_fields(chore: &Chore) -> Vec<String> {
    let mut fields = Vec::new();
    if !chore.deps.is_empty() {
        let items: Vec<String> = chore
            .deps
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("requires = {{{}}}", items.join(", ")));
    }
    fields
}

