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

    /// Every path or pattern the recipes in `recipe_names` read, taken from
    /// the registered units rather than from the root Cookfile's AST.
    ///
    /// # Why not the AST (COOK-407)
    ///
    /// This used to walk `cookfile.recipes` and collect each one's surface
    /// `ingredients`. That saw only recipes written as `recipe NAME` blocks in
    /// the ENTRY Cookfile, so three kinds of input were never watched:
    ///
    /// - anything a module registered (`cook_cc.bin` and friends mint their
    ///   units through `cook.add_unit`, and have no AST recipe at all),
    /// - every imported member's recipes, since only the root AST was walked,
    /// - a unit's declared `inputs`, as distinct from the recipe's
    ///   `ingredients` fan-out list.
    ///
    /// So on a C++ project `cook serve` watched nothing and rebuilt on no
    /// edit, while reporting itself as watching. `cook serve` was the one
    /// command that never received the unified `RegisteredWorkspace`
    /// treatment; `CacheMeta::inputs` is exactly "what this unit reads", which
    /// is the same question the watcher is asking.
    ///
    /// Paths are anchored to the owning recipe's `working_dir`, because an
    /// imported member's inputs are relative to that member's directory, not
    /// to the entry Cookfile's.
    pub fn collect_globs_for_recipes(
        registered: &cook_engine::RegisteredWorkspace,
        recipe_names: &[String],
    ) -> Vec<String> {
        let mut globs = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for name in recipe_names {
            let Some(units) = registered.units_by_recipe.get(name) else {
                continue;
            };
            for unit in &units.units {
                let Some(meta) = &unit.cache_meta else { continue };
                for input in &meta.inputs {
                    // Absolute patterns are already anchored; relative ones
                    // belong to the recipe that declared them.
                    let anchored = if Path::new(&input.path).is_absolute() {
                        input.path.clone()
                    } else {
                        units
                            .working_dir
                            .join(&input.path)
                            .to_string_lossy()
                            .into_owned()
                    };
                    if seen.insert(anchored.clone()) {
                        globs.push(anchored);
                    }
                }
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
