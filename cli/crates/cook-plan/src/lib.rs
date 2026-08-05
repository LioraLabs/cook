//! cook-plan — turn an invocation into a `RegisteredWorkspace` plus a
//! recipe edge map.
//!
//! This crate owns everything between "the user gave me a path" and "the
//! engine has every input it needs to start the unified work-unit DAG":
//! entry-point discovery, workspace import resolution, Cookfile parsing and
//! codegen, the unified register phase, and recipe-graph edge computation.
//! Under SHI-222 (CS-0077), `register_workspace` in [`registers`] — driven
//! by a [`RegisterMode`] that names the dispatch/introspect/enumerate
//! target semantics — is the only register-phase pipeline entry point;
//! `cook-cli` consumes it directly.
//! A single-Cookfile project (no imports) is a workspace of one member —
//! there is no separate single-Cookfile code path.
//!
//! It does not touch CLI-specific concerns (clap, terminal rendering, exit
//! codes) — those stay in `cook-cli` — and it spends no work: executing the
//! planned units is `cook-engine`'s walk, which consumes this crate's output
//! value without depending on this crate. The split lets non-CLI consumers
//! (the spec conformance harness, future LSPs, library embeddings) drive the
//! same orchestration without reimplementing it.
//!
//! ## Boundary
//!
//! | Concern | Owner |
//! |---|---|
//! | Cookfile parsing & codegen | [`parse`] |
//! | Entry-point / workspace-root anchoring | [`entry`] |
//! | Workspace import resolution | [`workspace`] |
//! | `.env` + `--set` env layering | [`env`] |
//! | Recipe-graph algorithms & namespace prefixes | [`analyzer`] |
//! | `RecipeInfo` map assembly | [`recipe_info`] |
//! | Unified register-phase entry | [`registers`] |
//! | `{NAME}` inferred-dep computation | [`inferred_deps`] |
//! | Pipeline-layer error type | [`error`] |
//!
//! Errors at this layer surface as `PipelineError`; the CLI maps it onto
//! its `CookError` for display + exit-code mapping.

pub mod analyzer;
pub mod entry;
pub mod env;
pub mod error;
pub mod inferred_deps;
pub mod parse;
pub mod recipe_info;
pub mod registers;
pub mod workspace;

pub use env::parse_cli_overrides;
pub use error::PipelineError;
pub use inferred_deps::{compute_workspace_inferred_deps, workspace_dep_conflicts};
pub use parse::{read_and_parse, validate_selected_config_workspace, ParsedCookfile};
pub use recipe_info::{build_recipe_infos_from_registered, find_full_prefix};
pub use registers::{
    codegen_with_module_recipes, list_workspace_names, register_workspace, RegisterMode,
};
pub use entry::{discover_entry_cookfile, resolve_workspace_root};
pub use workspace::{LoadedCookfile, Workspace};

// The value handed across the boundary: the plan produces it, the engine's
// walk consumes it. Re-exported so a caller driving the full plan → run
// sequence can name it from either side.
pub use cook_register::RegisteredWorkspace;
