//! Dynamic shell completion.
//!
//! Completion is driven by `clap_complete`'s dynamic engine rather than
//! generated static scripts: the candidate logic lives here once, in Rust, and
//! every supported shell calls back into the binary. Registration is by
//! environment variable (`COMPLETE=fish cook | source`), so no `completions`
//! subcommand is minted — which matters, because a new subcommand would grow
//! the reserved set and force `cook +completions` on anyone with a recipe by
//! that name.
//!
//! Two shapes of the CLI drive the design:
//!
//! * Recipes reach the parser through `Cmd::Recipe`'s `external_subcommand`
//!   catch-all, and the dynamic engine cannot complete external subcommands.
//!   `completion_command` therefore returns an *augmented* `Command` — recipes
//!   injected as real subcommands — used only for completion. The parse tree in
//!   `cli.rs` is untouched.
//! * The namespace differs per subcommand: a bare target accepts recipes *and*
//!   chores, `cook test` accepts recipes and namespace prefixes but never
//!   chores, and the rest accept recipe names only. Completing all of them from
//!   one list would be wrong far more often than right.
//!
//! Every lookup is best-effort: a Cookfile that fails to parse, a missing
//! module, or a name collision yields no candidates rather than an error, and
//! nothing here writes to stderr — a diagnostic printed mid-completion would
//! corrupt the shell's display.

use std::ffi::OsStr;
use std::path::PathBuf;

use clap::{Arg, Command, CommandFactory};
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use clap_complete::CompleteEnv;
use cook_engine::cook_register::RecipeKind;
use cook_engine::pipeline::{self, Workspace};

use crate::cli::{Cli, Globals};

/// Subcommand spellings that shadow a recipe of the same name, forcing the `+`
/// escape (`cook +test`). Kept as a constant because the reserved set must be
/// queried by string — `Cmd::built_in_name` can only answer for an already
/// parsed `Cmd`. `help` is clap's, not ours, and is easy to forget.
///
/// `reserved_names_match_the_parse_tree` pins this against the real command.
const RESERVED: &[&str] = &[
    "init", "menu", "list", "modules", "test", "dag", "logs", "cache", "serve", "emit-lua",
    "affected", "why", "help",
];

/// Entry point. Returns immediately unless the shell is driving completion.
pub fn complete() {
    // `CompleteEnv` uses the factory for two different jobs: emitting the
    // registration script (`COMPLETE=fish cook`, run once per shell startup)
    // and answering a candidate request (`COMPLETE=fish cook -- cook bu`).
    // Registration needs nothing but the binary's name, so it must NOT get the
    // augmented command — building that loads the workspace, and a shell
    // sourcing its startup file would then execute the register-phase Lua of
    // whatever directory it happened to start in. Only a candidate request
    // carries `--`.
    let is_candidate_request = std::env::args_os().any(|arg| arg == "--");
    if is_candidate_request {
        CompleteEnv::with_factory(completion_command).complete();
    } else {
        CompleteEnv::with_factory(Cli::command).complete();
    }
}

/// The `Command` used *only* to answer completion requests.
fn completion_command() -> Command {
    let mut cmd = Cli::command();

    // Teach the target-typed args their namespaces. Done by mutation rather
    // than `add = ...` attributes in cli.rs so that the parse tree carries no
    // completion coupling.
    cmd = cmd
        .mut_subcommand("test", |c| {
            c.mut_arg("scope", |a| {
                a.add(ArgValueCompleter::new(complete_test_scope))
            })
        })
        .mut_subcommand("dag", with_recipe_and_bare_preset)
        .mut_subcommand("why", with_recipe_and_bare_preset)
        .mut_subcommand("serve", with_recipe_and_bare_preset)
        .mut_subcommand("cache", |c| {
            c.mut_subcommand("verify", with_recipe_and_bare_preset)
        });

    for (name, kind) in workspace_names() {
        let typed = if RESERVED.contains(&name.as_str()) {
            format!("+{name}")
        } else {
            name
        };
        // A recipe shadowed by a builtin already occupies the bare spelling as
        // a real subcommand; only the `+`-escaped form is ours to add.
        if cmd.find_subcommand(&typed).is_some() {
            continue;
        }
        let mut sub = Command::new(typed);
        sub = match kind {
            // After a bare recipe a preset is selected with `@NAME` — a bare
            // positional is a chore param and is rejected outright.
            RecipeKind::Recipe => sub
                .about("recipe")
                .arg(Arg::new("preset").add(ArgValueCompleter::new(complete_sigil_preset))),
            // Chores take `name=value` params, not presets; offering presets
            // here would suggest invocations that do not typecheck.
            RecipeKind::Chore => sub.about("chore"),
        };
        cmd = cmd.subcommand(sub);
    }
    cmd
}

fn with_recipe_and_bare_preset(c: Command) -> Command {
    // These four, unlike a bare recipe, really do take the preset as a bare
    // second positional (`cook why build release`).
    c.mut_arg("recipe", |a| {
        a.add(ArgValueCompleter::new(complete_recipe_only))
    })
    .mut_arg("config", |a| {
        a.add(ArgValueCompleter::new(complete_bare_preset))
    })
}

// ---------------------------------------------------------------------------
// Candidate sources
// ---------------------------------------------------------------------------

/// Globals standing in for the ones clap would have parsed. Completion runs
/// before parsing, so `-f/--file` is not available; mirror `main`'s upward
/// discovery instead. `Globals::default` cannot be used wholesale — the
/// `Cookfile` default lives in a clap attribute, not in `Default`.
fn completion_globals() -> Option<Globals> {
    let mut globals = Globals {
        file: PathBuf::from("Cookfile"),
        ..Default::default()
    };
    if !globals.file.is_file() {
        let cwd = std::env::current_dir().ok()?;
        globals.file = pipeline::discover_entry_cookfile(&cwd, None).ok()?;
    }
    Some(globals)
}

/// True for names a module synthesised for its own bookkeeping rather than for
/// a human to type — `cook_cc`'s `cc.config_header`, for instance, registers
/// `__cc_config_header__<sanitized output>` purely so callers can declare a
/// `requires` against it, and hands the name back through Lua:
///
/// ```lua
/// local cfg = cc.config_header(template, output, vars)
/// cc.bin("game", { sources = {...}, requires = { cfg } })
/// ```
///
/// Nothing reaches such a recipe by typing its name, so suggesting it is noise
/// — in dhewm3 it is the *first* thing `cook <TAB>` would otherwise offer. They
/// stay runnable; they are merely not proposed. The cost is that a recipe
/// deliberately named `__foo` is also hidden.
///
/// Tested against the last segment so a workspace-qualified `game.__cc_…` is
/// caught as well as a bare one. Note `cook list` still prints these — this is
/// a completion-presentation choice, not a change to the listing surface.
fn is_module_internal(name: &str) -> bool {
    name.rsplit('.')
        .next()
        .is_some_and(|segment| segment.starts_with("__"))
}

/// Every registered name in the workspace worth proposing, with its kind.
///
/// This is `cook list`'s path (parse + codegen + a register pass per member,
/// no recipe bodies and no probe queries) — a few milliseconds even on a large
/// workspace, so no name cache is warranted. Note it does execute the
/// Cookfile's top-level Lua, which is what makes module-registered recipes
/// visible.
fn workspace_names() -> Vec<(String, RecipeKind)> {
    let Some(globals) = completion_globals() else {
        return Vec::new();
    };
    let Ok(root) = pipeline::resolve_workspace_root(&globals.file, globals.root.clone()) else {
        return Vec::new();
    };
    let Ok(workspace) = Workspace::load(&globals.file, &root, &globals.set) else {
        return Vec::new();
    };
    let Ok(names) = pipeline::list_workspace_names(&workspace, None, &globals.set) else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter(|r| !is_module_internal(&r.name))
        .map(|r| (r.name, r.kind))
        .collect()
}

/// Selectable presets, which are file-level and global to the entry Cookfile —
/// there is no recipe-to-preset relation to model. Parse-only, so cheaper than
/// `workspace_names`. The unnamed base `config` block has no name and is not
/// selectable.
fn preset_names() -> Vec<String> {
    let Some(globals) = completion_globals() else {
        return Vec::new();
    };
    let Ok(source) = std::fs::read_to_string(&globals.file) else {
        return Vec::new();
    };
    let Ok(cookfile) = cook_lang::parse(&source) else {
        return Vec::new();
    };
    cookfile
        .config_blocks
        .iter()
        .filter_map(|b| b.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Completers
// ---------------------------------------------------------------------------

fn filtered(current: &OsStr, candidates: Vec<(String, &'static str)>) -> Vec<CompletionCandidate> {
    let current = current.to_string_lossy();
    candidates
        .into_iter()
        .filter(|(value, _)| value.starts_with(current.as_ref()))
        .map(|(value, help)| CompletionCandidate::new(value).help(Some(help.into())))
        .collect()
}

/// Recipes only — `dag`, `serve`, `why`, `cache verify` reject chores and
/// namespaces alike.
fn complete_recipe_only(current: &OsStr) -> Vec<CompletionCandidate> {
    let candidates = workspace_names()
        .into_iter()
        .filter(|(_, kind)| matches!(kind, RecipeKind::Recipe))
        .map(|(name, _)| (name, "recipe"))
        .collect();
    filtered(current, candidates)
}

/// `cook test` resolves a recipe name or a namespace prefix, and excludes
/// chores. The prefixes are synthesised here: a namespace is implied by a
/// dotted recipe name and never appears as a name of its own.
fn complete_test_scope(current: &OsStr) -> Vec<CompletionCandidate> {
    let recipes: Vec<String> = workspace_names()
        .into_iter()
        .filter(|(_, kind)| matches!(kind, RecipeKind::Recipe))
        .map(|(name, _)| name)
        .collect();

    let mut namespaces: Vec<String> = recipes
        .iter()
        .flat_map(|name| {
            let segments: Vec<&str> = name.split('.').collect();
            (1..segments.len()).map(move |i| segments[..i].join("."))
        })
        .collect();
    namespaces.sort();
    namespaces.dedup();

    let candidates = recipes
        .into_iter()
        .map(|name| (name, "recipe"))
        .chain(namespaces.into_iter().map(|ns| (ns, "namespace")))
        .collect();
    filtered(current, candidates)
}

/// `@NAME`, the sigil form required after a bare recipe. The candidate carries
/// the sigil because that is what the user must type.
fn complete_sigil_preset(current: &OsStr) -> Vec<CompletionCandidate> {
    let candidates = preset_names()
        .into_iter()
        .map(|name| (format!("@{name}"), "preset"))
        .collect();
    filtered(current, candidates)
}

fn complete_bare_preset(current: &OsStr) -> Vec<CompletionCandidate> {
    let candidates = preset_names()
        .into_iter()
        .map(|name| (name, "preset"))
        .collect();
    filtered(current, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RESERVED` is hand-maintained but must equal the real reserved set: a
    /// name missing here would be completed bare, and the shell would silently
    /// dispatch the builtin instead of the user's recipe.
    #[test]
    fn reserved_names_match_the_parse_tree() {
        let cmd = Cli::command();
        let mut actual: Vec<String> = cmd
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .collect();
        actual.push("help".to_string());
        actual.sort();

        let mut expected: Vec<String> = RESERVED.iter().map(|s| s.to_string()).collect();
        expected.sort();

        assert_eq!(
            actual, expected,
            "reserved subcommand set drifted; update RESERVED in completion.rs"
        );
    }

    #[test]
    fn module_internal_names_are_recognised_including_when_qualified() {
        assert!(is_module_internal("__cc_config_header__build_config_h"));
        // A workspace member prefix must not hide the `__`.
        assert!(is_module_internal(
            "game.__cc_config_header__build_config_h"
        ));

        assert!(!is_module_internal("build"));
        assert!(!is_module_internal("cli.build"));
        // A single underscore is an ordinary name.
        assert!(!is_module_internal("_private"));
        // `__` must be at the start of the segment, not merely present.
        assert!(!is_module_internal("my__recipe"));
    }

    #[test]
    fn completion_command_does_not_disturb_the_parse_tree() {
        // The augmented command is completion-only; the real tree must still
        // route an unknown positional to the external_subcommand arm.
        let cmd = Cli::command();
        assert!(cmd.find_subcommand("why").is_some());
        assert!(cmd.find_subcommand("build").is_none());
    }
}
