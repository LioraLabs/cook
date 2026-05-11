//! Terminal UI viewer for Cook build logs.
//!
//! See `docs/superpowers/specs/2026-05-10-cook-logs-tui-design.md`.

pub mod error;
pub mod input;
pub mod render;
pub mod state;
pub mod theme;
pub mod tui;

pub use error::ViewerError;
pub use state::{Filter, Focus, UiState};
pub use theme::Theme;
pub use tui::{run, run_with_backend};

// TODO(task 2): restore cmd_logs, BuildSelector, resolve_selector, and
// nearby_ids once cook_progress::log_reader (BuildSummary, BuildView,
// LoadDiagnostics, list_builds, load) is added by the next task.
//
// use std::path::Path;
// use cook_progress::log_reader::{self, BuildSummary};
//
// pub fn cmd_logs(
//     project_root: &Path,
//     selector: BuildSelector,
//     theme: Theme,
// ) -> Result<(), ViewerError> { ... }
//
// #[derive(Debug, Clone)]
// pub enum BuildSelector {
//     Latest,
//     Nth(usize),
//     ByBuildId(String),
//     LastFailed,
// }
//
// fn resolve_selector(builds: &[BuildSummary], sel: &BuildSelector) -> Result<String, ViewerError> { ... }
// fn nearby_ids(builds: &[BuildSummary]) -> Vec<String> { ... }
