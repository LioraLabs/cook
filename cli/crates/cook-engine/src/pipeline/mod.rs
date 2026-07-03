//! Pipeline orchestration: load Cookfile(s) → run the unified register
//! phase → hand off to `crate::run::run`.
//!
//! Under SHI-222 (CS-0077), the per-prefix `RegistryEntry` map model and
//! the transitional shim that bridged it into the unified `run` entry
//! point are both retired. `register_workspace` / `register_single_cookfile`
//! in [`registers`] are the only register-phase pipeline entry points;
//! `cook-cli` consumes them directly.
//!
//! This module owns everything between "the user gave me a path" and "the
//! engine has every input it needs to start the unified work-unit DAG".
//! It does not touch CLI-specific concerns (clap, terminal rendering,
//! exit codes) — those stay in `cook-cli`. The split lets non-CLI
//! consumers (the spec conformance harness, future LSPs, library
//! embeddings) drive the same orchestration without reimplementing it.
//!
//! ## Boundary
//!
//! | Concern | Owner |
//! |---|---|
//! | Cookfile parsing & codegen | `pipeline::parse` |
//! | Workspace import resolution | `pipeline::workspace` |
//! | `.env` + `--set` env layering | `pipeline::env` |
//! | `RecipeInfo` map assembly | `pipeline::recipe_info` |
//! | Unified register-phase entry | `pipeline::registers` |
//! | `{NAME}` inferred-dep computation | `pipeline::inferred_deps` |
//! | Pipeline-layer error type | `pipeline::error` |
//!
//! Errors at this layer surface as `PipelineError`; the CLI maps it onto
//! its `CookError` for display + exit-code mapping.

pub mod env;
pub mod error;
pub mod inferred_deps;
pub mod parse;
pub mod recipe_info;
pub mod registers;
pub mod workspace;

pub use env::{load_env, parse_cli_overrides, resolve_env};
pub use error::PipelineError;
pub use inferred_deps::{
    compute_single_inferred_deps, compute_workspace_inferred_deps, single_dep_conflicts,
    workspace_dep_conflicts,
};
pub use parse::{read_and_parse, validate_selected_config, ParsedCookfile};
pub use recipe_info::{build_recipe_infos_from_registered, find_full_prefix};
pub use registers::{
    codegen_with_module_recipes, codegen_with_module_recipes_single, list_single_cookfile_names,
    list_workspace_names, register_single_cookfile, register_single_cookfile_with_argv,
    register_workspace, register_workspace_with_argv,
};
pub use workspace::{resolve_workspace_root, LoadedCookfile, Workspace};
