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

use std::path::Path;
use cook_progress::log_reader::{self, BuildSummary};

pub fn cmd_logs(
    project_root: &Path,
    selector: BuildSelector,
    theme: Theme,
) -> Result<(), ViewerError> {
    let logs_root = project_root.join(".cook").join("logs");
    let builds = log_reader::list_builds(&logs_root).map_err(ViewerError::io_listing)?;

    let build_id = resolve_selector(&builds, &selector)?;
    let build_dir = logs_root.join(&build_id);
    let (view, diag) = log_reader::load(&build_dir).map_err(|e| ViewerError::Load {
        build_id: build_id.clone(),
        source: e,
    })?;
    run(view, diag, theme)
}

#[derive(Debug, Clone)]
pub enum BuildSelector {
    Latest,
    Nth(usize),
    ByBuildId(String),
    LastFailed,
}

fn resolve_selector(
    builds: &[BuildSummary],
    sel: &BuildSelector,
) -> Result<String, ViewerError> {
    match sel {
        BuildSelector::Latest => builds
            .first()
            .map(|b| b.build_id.clone())
            .ok_or_else(|| ViewerError::BuildNotFound {
                requested: "latest".to_string(),
                nearby: Vec::new(),
            }),
        BuildSelector::Nth(n) => {
            let idx = n.saturating_sub(1);
            builds.get(idx).map(|b| b.build_id.clone()).ok_or_else(|| {
                ViewerError::BuildNotFound {
                    requested: format!("nth={n}"),
                    nearby: nearby_ids(builds),
                }
            })
        }
        BuildSelector::ByBuildId(id) => {
            if builds.iter().any(|b| &b.build_id == id) {
                Ok(id.clone())
            } else {
                Err(ViewerError::BuildNotFound {
                    requested: id.clone(),
                    nearby: nearby_ids(builds),
                })
            }
        }
        BuildSelector::LastFailed => builds
            .iter()
            .find(|b| b.exit_code.map(|c| c != 0).unwrap_or(false))
            .map(|b| b.build_id.clone())
            .ok_or_else(|| ViewerError::BuildNotFound {
                requested: "last-failed".to_string(),
                nearby: nearby_ids(builds),
            }),
    }
}

fn nearby_ids(builds: &[BuildSummary]) -> Vec<String> {
    builds.iter().take(5).map(|b| b.build_id.clone()).collect()
}
