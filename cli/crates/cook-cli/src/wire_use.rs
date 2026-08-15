//! Making an installed module reachable: the `use` declaration a named
//! `cook modules install` writes into the project's Cookfile (§27.1.1,
//! CS-0220).
//!
//! Installing a module and declaring it are one act with two halves, and only
//! the first half used to happen. A module the Cookfile does not `use` is on
//! disk, in the lockfile, listed by `cook modules list`, and completely inert:
//! its makers are unreachable and the chores it registers under §22.12 cannot
//! be dispatched. The author naming the module IS the request to use it, so a
//! second request carries no information — the `cargo add` precedent, and the
//! reason the hand-edit this removes sat at minute zero of every bootstrap.
//!
//! # Why this lives in `cook-cli` and not in `cook-modules`
//!
//! The verb is `cook modules install`, so the obvious home is the crate that
//! implements it. The stratum table refuses it: `cook-modules` and
//! `cook-cookfile` are both *mechanism*, and a sideways edge is exactly what
//! `cook-contracts`' constitution forbids. The seam it forces is the honest
//! one anyway. `cook-modules` answers what a rock is and which ones an
//! invocation named; writing the author's project files is a job this crate
//! already owns for `cook init`, one function above in the same directory.
//!
//! # Which Cookfile
//!
//! The one in the invocation directory, not the one upward discovery would
//! find. `cook modules` is exempted from entry discovery (`main.rs`,
//! `apply_entry_discovery`) because it manages the `cook.toml` and `cook.lock`
//! of the directory it was run in, and the declaration has to land in the
//! Cookfile that belongs to those two files. `cook init` anchors the same way
//! for the same reason.
//!
//! # The decision is separated from the doing
//!
//! [`plan_wiring`] is a pure function from the file's current bytes to the
//! bytes it should hold and the lines to report; [`wire_use_declarations`]
//! reads, writes and prints. That split is what lets every case — created,
//! inserted, already present, unspellable name, unparseable file — be pinned
//! without a filesystem.

use std::path::Path;

use cook_contracts::module_binding::is_use_name;
use cook_cookfile::{ensure_use, EditError, UseEdit};

use crate::error::CookError;
use crate::pipeline::{merge_cook_gitignore_section, GitignoreMerge};

/// What wiring a set of module names into a Cookfile comes to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Wiring {
    /// The file's new contents, or `None` when nothing needs writing — every
    /// requested declaration was already there, or none could be written at
    /// all.
    pub source: Option<String>,
    /// The modules whose declaration this wiring adds, in the order asked for.
    pub added: Vec<String>,
    /// The modules that are installed and cannot be declared, because a rock
    /// name is drawn from a wider alphabet than a `use` name is (§5.1).
    pub unspellable: Vec<String>,
}

/// Decide what `existing` should become once `names` are declared in it.
///
/// `existing` is `None` for a project with no Cookfile at all, which is the
/// bootstrap case and is deliberately not special-cased: an absent file plans
/// exactly as an empty one, so the file this creates is the file an empty one
/// would have become. What the caller does differently is create it, and say
/// so.
///
/// A name that is not a legal `use` name is dropped from the edit rather than
/// rewritten. `cook_contracts::module_binding::alias_of` would map `foo-bar`
/// to `foo_bar`, and writing that declares a module that was never installed:
/// a Cookfile that parses, names something plausible, and fails at load over a
/// name the author never asked for.
pub fn plan_wiring(existing: Option<&str>, names: &[String]) -> Result<Wiring, EditError> {
    let mut source = existing.unwrap_or_default().to_string();
    let mut plan = Wiring::default();
    let mut edited = false;

    for name in names {
        if !is_use_name(name) {
            plan.unspellable.push(name.clone());
            continue;
        }
        match ensure_use(&source, name)? {
            UseEdit::AlreadyPresent => {}
            UseEdit::Inserted(next) => {
                source = next;
                edited = true;
                plan.added.push(name.clone());
            }
        }
    }

    // A file is written only when the edit produced different bytes. Leaving
    // the already-present case byte-identical and rewriting identical bytes
    // are not the same thing: the second moves the mtime, which an editor and
    // a build tool both watch.
    if edited {
        plan.source = Some(source);
    }
    Ok(plan)
}

/// The file's contents, or `None` when it does not exist.
///
/// Absent and unreadable are kept apart: both files this module touches are
/// created when missing, and a permission error read as "missing" would
/// overwrite a file it could not open.
fn read_if_present(path: &Path) -> Result<Option<String>, CookError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CookError::Other(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// Write the `use` declarations for `names` into `project_dir`'s Cookfile,
/// creating the Cookfile (and merging the managed `.gitignore` section) when
/// there is none, and report what changed.
///
/// Called after the install has succeeded, so everything reported here is
/// about a module already on disk. That ordering is why an unspellable name is
/// a warning rather than a failure: the rock is installed and recorded either
/// way, and refusing the install over a defect in a convenience would be the
/// larger failure. An unparseable Cookfile is the one case that does fail the
/// command, because there the thing the author asked for demonstrably has not
/// happened and no later step will notice.
pub fn wire_use_declarations(project_dir: &Path, names: &[String]) -> Result<(), CookError> {
    let cookfile = project_dir.join("Cookfile");
    let existing = read_if_present(&cookfile)?;
    let creating = existing.is_none();

    let plan = plan_wiring(existing.as_deref(), names).map_err(|e| {
        // The only failure this call can raise is an unparseable file: it adds
        // a top-level declaration, so there is no recipe, call or field it
        // could fail to locate. The file is untouched either way, and the
        // author is told which lines are missing from it.
        let missing = names
            .iter()
            .filter(|n| is_use_name(n))
            .map(|n| format!("`use {n}`"))
            .collect::<Vec<_>>()
            .join(", ");
        CookError::Other(format!(
            "{}: {e}. {missing} was not added; fix the file and add it yourself",
            cookfile.display()
        ))
    })?;

    for name in &plan.unspellable {
        eprintln!(
            "cook: '{name}' is installed but cannot be declared — a `use` name is \
             [A-Za-z_][A-Za-z0-9_]* and this one is not, so no line was written to the Cookfile"
        );
    }

    let Some(source) = plan.source else {
        return Ok(());
    };
    std::fs::write(&cookfile, source)
        .map_err(|e| CookError::Other(format!("failed to write {}: {e}", cookfile.display())))?;

    if creating {
        let declared = plan
            .added
            .iter()
            .map(|n| format!("'use {n}'"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("Created Cookfile with {declared}");
        merge_gitignore(project_dir)?;
    } else {
        for name in &plan.added {
            println!("Cookfile: added 'use {name}'");
        }
    }
    Ok(())
}

/// Apply the managed `.gitignore` section, by the same merge `cook init` uses.
///
/// Only reached when this command created the Cookfile, and the install that
/// preceded it has just populated `.cook/`. Leaving the project's first
/// tracked file behind without the ignore rules hands the author a repository
/// whose next `git add` commits a module tree.
fn merge_gitignore(project_dir: &Path) -> Result<(), CookError> {
    let path = project_dir.join(".gitignore");
    let existing = read_if_present(&path)?;
    let (content, said) = match merge_cook_gitignore_section(existing.as_deref()) {
        GitignoreMerge::Unchanged => return Ok(()),
        GitignoreMerge::Created(content) => (content, "Created .gitignore"),
        GitignoreMerge::Appended(content) => (content, "Updated .gitignore with Cook entries"),
    };
    std::fs::write(&path, content)
        .map_err(|e| CookError::Other(format!("failed to write {}: {e}", path.display())))?;
    println!("{said}");
    Ok(())
}

#[cfg(test)]
#[path = "tests/wire_use_tests.rs"]
mod tests;
