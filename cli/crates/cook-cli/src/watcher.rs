//! File watcher for `cook serve`.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct CookWatcher {
    pub globs: Vec<String>,
    pub cookfile_paths: Vec<PathBuf>,
}

impl CookWatcher {
    pub fn new(globs: Vec<String>, cookfile_paths: Vec<PathBuf>) -> Self {
        Self {
            globs,
            cookfile_paths,
        }
    }

    pub fn collect_globs_for_recipes(
        cookfile: &cook_lang::ast::Cookfile,
        recipe_names: &[String],
    ) -> Vec<String> {
        let mut globs = Vec::new();
        for recipe in &cookfile.recipes {
            if recipe_names.contains(&recipe.name) {
                globs.extend(recipe.ingredients.clone());
            }
        }
        globs
    }

    fn matches_any_glob(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in &self.globs {
            if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                if glob_pattern.matches(&path_str) {
                    return true;
                }
            }
        }
        false
    }

    pub fn watch<F>(&self, on_change: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: Fn(bool) -> Result<(), Box<dyn std::error::Error>>,
    {
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            notify::Config::default(),
        )?;

        let mut watched_dirs = std::collections::HashSet::new();
        for pattern in &self.globs {
            let dir = Path::new(pattern).parent().unwrap_or(Path::new("."));
            if watched_dirs.insert(dir.to_path_buf()) && dir.exists() {
                watcher.watch(dir, RecursiveMode::Recursive)?;
            }
        }

        for cookfile_path in &self.cookfile_paths {
            if let Some(cookfile_dir) = cookfile_path.parent() {
                if watched_dirs.insert(cookfile_dir.to_path_buf()) && cookfile_dir.exists() {
                    watcher.watch(cookfile_dir, RecursiveMode::NonRecursive)?;
                }
            }
        }

        let debounce = Duration::from_millis(200);
        let mut last_trigger = Instant::now() - debounce;

        loop {
            match rx.recv() {
                Ok(event) => {
                    if Instant::now().duration_since(last_trigger) < debounce {
                        continue;
                    }

                    let cookfile_changed = event
                        .paths
                        .iter()
                        .any(|p| self.cookfile_paths.iter().any(|cp| p == cp));

                    let relevant =
                        cookfile_changed || event.paths.iter().any(|p| self.matches_any_glob(p));

                    if relevant {
                        last_trigger = Instant::now();
                        if let Err(e) = on_change(cookfile_changed) {
                            let msg = cook_cli::diagnostics::sanitize_error(
                                &e.to_string(),
                                cook_cli::diagnostics::backtrace_enabled(),
                            );
                            eprintln!("cook serve: rebuild failed: {msg}");
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("watch error: {e}").into());
                }
            }
        }
    }
}
