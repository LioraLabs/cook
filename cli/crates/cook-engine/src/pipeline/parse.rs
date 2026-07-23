//! Parse a Cookfile from disk into AST + Lua source, and validate
//! `--config` selection against the Cookfile's named config blocks.
//!
//! These two functions sit at the very top of the orchestration pipeline:
//! everything downstream (registries, recipe info, inferred deps) consumes
//! the AST + Lua source produced here.

use std::collections::BTreeSet;
use std::path::Path;

use cook_lang::ast::Cookfile;

use super::error::PipelineError;
use super::workspace::Workspace;

/// Result of `read_and_parse`: the parsed AST, the generated Lua source,
/// and any non-fatal warnings collected during codegen.
///
/// The CLI is responsible for printing `warnings` to stderr; the engine
/// returns them unconditionally so non-CLI consumers (the spec conformance
/// harness, future LSPs, etc.) can handle them however they like.
pub struct ParsedCookfile {
    pub cookfile: Cookfile,
    pub lua_source: String,
    pub warnings: Vec<String>,
}

/// Read a Cookfile from `path`, parse it, and run codegen. Returns the
/// AST, the generated Lua source, and any warnings collected during
/// codegen.
///
/// Codegen is run twice: once with placement validation enabled
/// (`generate_with_names_checked`) so § 5.4 violations become hard errors,
/// and once with warnings enabled (`generate_with_names_and_warnings`) so
/// § 5.5 warnings can be surfaced. The two passes operate on the same AST
/// and produce the same Lua source — only the validation/warning policy
/// differs.
pub fn read_and_parse(path: &Path) -> Result<ParsedCookfile, PipelineError> {
    let source = std::fs::read_to_string(path).map_err(|e| PipelineError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let cookfile = cook_lang::parse(&source).map_err(|e| PipelineError::Parse(e.to_string()))?;

    // Pre-scan: extract recipe names for codegen disambiguation
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);

    // § 5.4 — accessor placement validation rejects `{lib.ACCESSOR}` in
    // contexts that lack a matching driver in an output pattern.
    let lua_source = cook_luagen::generate_with_names_checked(&cookfile, &recipe_names)
        .map_err(|e| PipelineError::Codegen(e.to_string()))?;

    // § 5.5 — register-time warnings for references whose referent has an
    // empty output list.
    let (_, warnings) = cook_luagen::generate_with_names_and_warnings(&cookfile, &recipe_names);

    Ok(ParsedCookfile {
        cookfile,
        lua_source,
        warnings,
    })
}

/// Collect every named `config_block` name declared in `cookfile`.
fn named_configs(cookfile: &Cookfile) -> impl Iterator<Item = &str> {
    cookfile
        .config_blocks
        .iter()
        .filter_map(|b| b.name.as_deref())
}

/// Validate `@PRESET` selection against the UNION of the named config blocks of
/// every loaded Cookfile in `workspace` (root + all transitive imports), per
/// §11.6 / CS-0165 (R3). A name declared in ANY member passes; a name declared
/// in NO member is a hard error listing the deduplicated, sorted union of
/// available names. A member that simply lacks the selected overlay runs its
/// base config downstream (a silent no-op) — that is not this function's
/// concern; it only rejects names that exist nowhere.
///
/// This replaces the pre-CS-0165 entry-Cookfile-only check: the selected name
/// already propagates to every member's register builder, so validating only
/// the entry Cookfile spuriously rejected a preset declared in an import (the
/// sole reason `cook cli.build @profiling` failed while `cd cli && cook build
/// @profiling` worked).
pub fn validate_selected_config_workspace(
    workspace: &Workspace,
    config: Option<&str>,
) -> Result<(), PipelineError> {
    let Some(name) = config else {
        return Ok(());
    };
    let members = std::iter::once(&workspace.root).chain(workspace.imports.values());
    // Union of names across all members, deduplicated and ordered for a stable
    // diagnostic.
    let mut available: BTreeSet<String> = BTreeSet::new();
    for member in members {
        for n in named_configs(&member.cookfile) {
            if n == name {
                return Ok(());
            }
            available.insert(n.to_string());
        }
    }
    Err(PipelineError::UnknownConfig {
        name: name.to_string(),
        available: available.into_iter().collect(),
    })
}
