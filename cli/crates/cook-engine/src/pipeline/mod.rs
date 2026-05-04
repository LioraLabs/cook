//! Pipeline orchestration: load Cookfile(s) → assemble registries → infer
//! deps → hand off to `crate::run::run`.
//!
//! This module owns everything between "the user gave me a path" and "the
//! engine has every input it needs to start the wave loop". It does not
//! touch CLI-specific concerns (clap, terminal rendering, exit codes) —
//! those stay in `cook-cli`. The split lets non-CLI consumers (the spec
//! conformance harness, future LSPs, library embeddings) drive the same
//! orchestration without reimplementing it.
//!
//! ## Boundary
//!
//! | Concern | Owner |
//! |---|---|
//! | Cookfile parsing & codegen | `pipeline::parse` |
//! | Workspace import resolution | `pipeline::workspace` |
//! | `.env` + `--set` env layering | `pipeline::env` |
//! | `RecipeInfo` map assembly | `pipeline::recipe_info` |
//! | `RegistryEntry` map assembly | `pipeline::registries` |
//! | `{NAME}` inferred-dep computation | `pipeline::inferred_deps` |
//! | DAG-unit collection (for viewer) | `pipeline::dag_units` |
//! | Pipeline-layer error type | `pipeline::error` |
//!
//! Errors at this layer surface as `PipelineError`; the CLI maps it onto
//! its `CookError` for display + exit-code mapping.

pub mod dag_units;
pub mod env;
pub mod error;
pub mod inferred_deps;
pub mod parse;
pub mod recipe_info;
pub mod registries;
pub mod workspace;

pub use dag_units::{collect_dag_units, DagUnits};
pub use env::{load_env, resolve_env};
pub use error::PipelineError;
pub use inferred_deps::{
    compute_single_inferred_deps, compute_workspace_inferred_deps, single_dep_conflicts,
    workspace_dep_conflicts,
};
pub use parse::{read_and_parse, validate_selected_config, ParsedCookfile};
pub use recipe_info::{build_single_recipe_infos, build_workspace_recipe_info, find_full_prefix};
pub use registries::{build_single_registries, build_workspace_registries};
pub use workspace::{resolve_workspace_root, LoadedCookfile, Workspace};
