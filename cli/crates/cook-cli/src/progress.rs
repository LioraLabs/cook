//! Progress renderer: ProgressEvent -> terminal output via cook-progress.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cook_progress::{Frame, ItemStatus, RenderConfig, Renderer as CookRenderer, Section, Status};

use crate::color::ColorConfig;

// ---------------------------------------------------------------------------
// ProgressEvent
// ---------------------------------------------------------------------------

/// Events emitted during build execution and consumed by the renderer.
///
/// NOTE: When cook-engine is available, this may move there. For now it lives
/// in cook-cli since the CLI owns the rendering pipeline.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ProgressEvent {
    RecipeQueued { name: String, total_nodes: usize },
    RecipeStarted { name: String, total_nodes: usize },
    RecipeCompleted { name: String, elapsed: Duration, cached_nodes: usize, total_nodes: usize },
    RecipeFailed { name: String, elapsed: Duration, completed_nodes: usize, total_nodes: usize },
    NodeStarted { recipe: String, node_name: String },
    NodeCompleted { recipe: String, node_name: String, elapsed: Duration },
    NodeFailed { recipe: String, node_name: String, elapsed: Duration, error: String },
    NodeCacheHit { recipe: String, node_name: String },
    NodeSkipped { recipe: String, node_name: String },
    OutputLine { recipe: String, line: String, is_stderr: bool },
    InteractiveStart { recipe: String },
    InteractiveEnd { recipe: String, elapsed: Duration, success: bool },
    TestResult { suite: String, test_name: String, passed: bool, elapsed: Duration, output: String },
    Finished,
}

// ---------------------------------------------------------------------------
// ProgressRenderer trait
// ---------------------------------------------------------------------------

pub trait ProgressRenderer: Send {
    fn handle(&mut self, event: ProgressEvent);
    fn drain(&self) -> Vec<u8>;
}

// ---------------------------------------------------------------------------
// PlainRenderer
// ---------------------------------------------------------------------------

/// Non-TTY renderer that outputs `[recipe] line` format to an internal buffer.
/// Used when cook's output is piped or redirected.
pub struct PlainRenderer {
    out: Arc<Mutex<Vec<u8>>>,
    #[allow(dead_code)]
    color: ColorConfig,
}

impl PlainRenderer {
    pub fn new(out: Arc<Mutex<Vec<u8>>>, color: ColorConfig) -> Self {
        PlainRenderer { out, color }
    }

    /// Drain and return the contents of the internal buffer.
    /// The caller is responsible for writing the returned bytes to stderr.
    pub fn drain_buf(&self) -> Vec<u8> {
        let mut guard = self.out.lock().unwrap();
        let contents = guard.clone();
        guard.clear();
        contents
    }

    fn write(&self, s: &str) {
        let mut guard = self.out.lock().unwrap();
        let _ = guard.write_all(s.as_bytes());
    }
}

impl ProgressRenderer for PlainRenderer {
    fn handle(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::RecipeQueued { .. } => {}
            ProgressEvent::RecipeStarted { .. } => {}

            ProgressEvent::OutputLine { recipe, line, .. } => {
                self.write(&format!("[{recipe}] {line}\n"));
            }

            ProgressEvent::RecipeCompleted { name, elapsed, cached_nodes, total_nodes } => {
                let secs = elapsed.as_secs_f64();
                let cache_suffix = if total_nodes > 0 && cached_nodes == total_nodes {
                    ", cached".to_string()
                } else if cached_nodes > 0 {
                    format!(", {cached_nodes}/{total_nodes} cached")
                } else {
                    String::new()
                };
                self.write(&format!("[{name}] done ({secs:.1}s{cache_suffix})\n"));
            }

            ProgressEvent::RecipeFailed { name, elapsed, completed_nodes, total_nodes } => {
                let secs = elapsed.as_secs_f64();
                self.write(&format!(
                    "[{name}] FAILED ({completed_nodes}/{total_nodes} steps, {secs:.1}s)\n"
                ));
            }

            ProgressEvent::NodeStarted { .. } => {}
            ProgressEvent::NodeCompleted { .. } => {}

            ProgressEvent::NodeFailed { recipe, error, .. } => {
                self.write(&format!("[{recipe}] {error}\n"));
            }

            ProgressEvent::NodeCacheHit { recipe, node_name } => {
                self.write(&format!("[{recipe}] cached: {node_name}\n"));
            }
            ProgressEvent::NodeSkipped { .. } => {}
            ProgressEvent::InteractiveStart { .. } => {}
            ProgressEvent::InteractiveEnd { .. } => {}
            ProgressEvent::TestResult { .. } => {}
            ProgressEvent::Finished => {}
        }
    }

    fn drain(&self) -> Vec<u8> {
        self.drain_buf()
    }
}

// ---------------------------------------------------------------------------
// TtyRenderer
// ---------------------------------------------------------------------------

/// Per-recipe rendering state tracked by TtyRenderer.
pub struct RecipeRenderState {
    pub total_nodes: usize,
    pub completed_nodes: usize,
    pub cached_nodes: usize,
    pub failed: bool,
    pub finished: bool,
    pub start: Instant,
    pub active_nodes: Vec<String>,
    pub cached_node_names: BTreeSet<String>,
    pub completed_node_names: BTreeMap<String, bool>,
    pub skipped_node_names: BTreeSet<String>,
    pub error_output: Vec<String>,
}

impl RecipeRenderState {
    fn new(total_nodes: usize) -> Self {
        RecipeRenderState {
            total_nodes,
            completed_nodes: 0,
            cached_nodes: 0,
            failed: false,
            finished: false,
            start: Instant::now(),
            active_nodes: Vec::new(),
            cached_node_names: BTreeSet::new(),
            completed_node_names: BTreeMap::new(),
            skipped_node_names: BTreeSet::new(),
            error_output: Vec::new(),
        }
    }
}

/// Terminal renderer using `cook-progress` for animated progress display.
///
/// Each call to `handle()` rebuilds the frame from current state and renders
/// it to stderr, using `cook-progress`'s clear/render cycle to animate in place.
pub struct TtyRenderer {
    renderer: CookRenderer,
    pub recipes: BTreeMap<String, RecipeRenderState>,
    recipe_order: Vec<String>,
    total_recipes: usize,
    finished_recipes: usize,
    running_recipes: usize,
    waiting_recipes: usize,
    total_cached: usize,
    total_nodes: usize,
}

impl TtyRenderer {
    /// Create a new TtyRenderer that writes to stderr via cook-progress.
    pub fn new(color: ColorConfig) -> Self {
        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
        let config = RenderConfig {
            width: cols,
            max_output_lines: 3,
            colors: color.enabled,
            ..Default::default()
        };
        TtyRenderer {
            renderer: CookRenderer::new(config),
            recipes: BTreeMap::new(),
            recipe_order: Vec::new(),
            total_recipes: 0,
            finished_recipes: 0,
            running_recipes: 0,
            waiting_recipes: 0,
            total_cached: 0,
            total_nodes: 0,
        }
    }

    /// Create a TtyRenderer for testing with a fixed width.
    #[allow(dead_code)]
    pub fn new_for_test(color: ColorConfig, width: u16) -> Self {
        let config = RenderConfig {
            width,
            max_output_lines: 3,
            colors: color.enabled,
            ..Default::default()
        };
        TtyRenderer {
            renderer: CookRenderer::new(config),
            recipes: BTreeMap::new(),
            recipe_order: Vec::new(),
            total_recipes: 0,
            finished_recipes: 0,
            running_recipes: 0,
            waiting_recipes: 0,
            total_cached: 0,
            total_nodes: 0,
        }
    }

    /// cook-progress writes directly to stderr, so drain returns empty.
    pub fn drain_buf(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Update internal state from a progress event.
    pub fn update_state(&mut self, event: &ProgressEvent) {
        match event {
            ProgressEvent::RecipeQueued { name, total_nodes } => {
                if !self.recipes.contains_key(name) {
                    self.recipes
                        .insert(name.clone(), RecipeRenderState::new(*total_nodes));
                    self.recipe_order.push(name.clone());
                }
                self.total_recipes += 1;
                self.waiting_recipes += 1;
                self.total_nodes += total_nodes;
            }

            ProgressEvent::RecipeStarted { name, total_nodes } => {
                if !self.recipes.contains_key(name) {
                    self.recipes
                        .insert(name.clone(), RecipeRenderState::new(*total_nodes));
                    self.recipe_order.push(name.clone());
                    self.total_nodes += total_nodes;
                } else {
                    // Was queued with total_nodes=0, now update with real count
                    let old_total = self.recipes[name].total_nodes;
                    if let Some(state) = self.recipes.get_mut(name) {
                        state.total_nodes = *total_nodes;
                    }
                    // Adjust global total_nodes (remove old, add new)
                    self.total_nodes = self.total_nodes - old_total + total_nodes;
                    if self.waiting_recipes > 0 {
                        self.waiting_recipes -= 1;
                    }
                }
                self.running_recipes += 1;
            }

            ProgressEvent::NodeStarted {
                recipe, node_name, ..
            } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.active_nodes.push(node_name.clone());
                }
            }

            ProgressEvent::NodeCompleted {
                recipe, node_name, ..
            } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.active_nodes.retain(|n| n != node_name);
                    state.completed_nodes += 1;
                    state.completed_node_names.insert(node_name.clone(), true);
                }
            }

            ProgressEvent::NodeFailed {
                recipe,
                node_name,
                error,
                ..
            } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.active_nodes.retain(|n| n != node_name);
                    state.failed = true;
                    state.completed_nodes += 1;
                    state.completed_node_names.insert(node_name.clone(), false);
                    state.error_output.push(error.clone());
                }
            }

            ProgressEvent::NodeCacheHit {
                recipe, node_name, ..
            } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.completed_nodes += 1;
                    state.cached_nodes += 1;
                    state.cached_node_names.insert(node_name.clone());
                }
                self.total_cached += 1;
            }

            ProgressEvent::NodeSkipped {
                recipe, node_name, ..
            } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.completed_nodes += 1;
                    state.skipped_node_names.insert(node_name.clone());
                }
            }

            ProgressEvent::OutputLine { .. } => {
                // Output buffering is handled by cook-progress renderer
            }

            ProgressEvent::RecipeCompleted {
                name,
                cached_nodes,
                ..
            } => {
                if let Some(state) = self.recipes.get_mut(name) {
                    state.finished = true;
                    state.cached_nodes = *cached_nodes;
                    state.active_nodes.clear();
                }
                self.finished_recipes += 1;
                if self.running_recipes > 0 {
                    self.running_recipes -= 1;
                }
            }

            ProgressEvent::RecipeFailed { name, .. } => {
                if let Some(state) = self.recipes.get_mut(name) {
                    state.finished = true;
                    state.failed = true;
                    state.active_nodes.clear();
                }
                self.finished_recipes += 1;
                if self.running_recipes > 0 {
                    self.running_recipes -= 1;
                }
            }

            // Events not tracked in render state
            ProgressEvent::InteractiveStart { .. }
            | ProgressEvent::InteractiveEnd { .. }
            | ProgressEvent::TestResult { .. }
            | ProgressEvent::Finished => {}
        }
    }

    /// Build a cook-progress Frame from current state.
    fn build_frame(&self) -> Frame {
        let mut frame = Frame::new();
        for name in &self.recipe_order {
            let state = match self.recipes.get(name) {
                Some(s) => s,
                None => continue,
            };
            let mut section = Section::new(name, name);
            if state.finished && !state.failed {
                let all_cached =
                    state.total_nodes > 0 && state.cached_nodes == state.total_nodes;
                if all_cached {
                    section = section
                        .status(Status::Cached)
                        .progress(state.total_nodes, state.total_nodes);
                } else {
                    section = section
                        .status(Status::Completed)
                        .progress(state.total_nodes, state.total_nodes)
                        .elapsed(state.start.elapsed());
                    if state.cached_nodes > 0 {
                        section = section.cache(state.cached_nodes, state.total_nodes);
                    }
                }
            } else if state.finished && state.failed {
                section = section
                    .status(Status::Failed)
                    .progress(state.completed_nodes, state.total_nodes)
                    .elapsed(state.start.elapsed());
            } else if state.active_nodes.is_empty() && state.completed_nodes == 0 {
                section = section.status(Status::Waiting);
            } else {
                section = section
                    .status(Status::Running)
                    .progress(state.completed_nodes, state.total_nodes)
                    .elapsed(state.start.elapsed());
                for node in &state.active_nodes {
                    section = section.active_item(node, ItemStatus::Running);
                }
            }
            frame = frame.section(section);
        }
        // Footer
        let mut parts: Vec<String> = Vec::new();
        if self.finished_recipes > 0 {
            parts.push(format!("{} done", self.finished_recipes));
        }
        if self.running_recipes > 0 {
            parts.push(format!("{} running", self.running_recipes));
        }
        if self.waiting_recipes > 0 {
            parts.push(format!("{} waiting", self.waiting_recipes));
        }
        if self.total_cached > 0 {
            parts.push(format!(
                "{}/{} cached",
                self.total_cached, self.total_nodes
            ));
        }
        if !parts.is_empty() {
            frame = frame.footer(parts.join(" \u{00b7} "));
        }
        frame
    }
}

impl ProgressRenderer for TtyRenderer {
    fn handle(&mut self, event: ProgressEvent) {
        let is_queue = matches!(event, ProgressEvent::RecipeQueued { .. });
        let is_interactive_start = matches!(event, ProgressEvent::InteractiveStart { .. });
        let is_finished = matches!(event, ProgressEvent::Finished);

        // Push output lines to cook-progress renderer
        if let ProgressEvent::OutputLine {
            ref recipe,
            ref line,
            ..
        } = event
        {
            self.renderer.push_output(recipe, line);
        }
        // Push error output and mark error
        if let ProgressEvent::NodeFailed {
            ref recipe,
            ref error,
            ..
        } = event
        {
            for err_line in error.lines() {
                self.renderer.push_output(recipe, err_line);
            }
            self.renderer.set_error(recipe);
        }

        self.update_state(&event);

        if is_queue {
            return;
        }

        if is_interactive_start {
            // Clear the progress display before handing over to interactive command
            let mut buf = Vec::new();
            let _ = self.renderer.clear_last_frame(&mut buf);
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &buf);
            self.renderer.reset();
            return;
        }

        // Follow the same clear -> render pattern as cook-progress examples:
        // clear_last_frame erases the previous frame, then render_frame draws
        // the new one. Buffered to a Vec for atomic write to stderr.
        let frame = self.build_frame();
        let mut buf = Vec::new();
        let _ = self.renderer.clear_last_frame(&mut buf);
        let _ = self.renderer.render_frame(&frame, &mut buf);
        if is_finished {
            buf.extend_from_slice(b"\n");
        }
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &buf);
    }

    fn drain(&self) -> Vec<u8> {
        self.drain_buf()
    }
}

/// Spawn a renderer thread that reads from `event_rx` and writes to stderr.
/// Selects TtyRenderer when stderr is a TTY, PlainRenderer otherwise.
/// Returns the join handle for the renderer thread.
pub fn spawn_renderer_thread(
    event_rx: std::sync::mpsc::Receiver<ProgressEvent>,
    color: ColorConfig,
) -> std::thread::JoinHandle<()> {
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    std::thread::spawn(move || {
        let mut renderer: Box<dyn ProgressRenderer> = if is_tty {
            Box::new(TtyRenderer::new(color))
        } else {
            let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
            Box::new(PlainRenderer::new(buf, color))
        };
        while let Ok(event) = event_rx.recv() {
            let is_finished = matches!(event, ProgressEvent::Finished);
            renderer.handle(event);
            let data = renderer.drain();
            if !data.is_empty() {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &data);
            }
            if is_finished {
                break;
            }
        }
    })
}

/// Resolve color configuration from CLI flag.
pub fn resolve_color(cli: &crate::cli::Cli) -> ColorConfig {
    use crate::color::ColorMode;
    let color_mode: ColorMode = cli.color.parse().unwrap_or(ColorMode::Auto);
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let no_color = std::env::var("NO_COLOR").is_ok();
    ColorConfig::resolve(color_mode, is_tty, no_color)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;

    #[test]
    fn test_progress_event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ProgressEvent>();
    }

    #[test]
    fn test_progress_event_clone() {
        let event = ProgressEvent::RecipeStarted {
            name: "build".to_string(),
            total_nodes: 5,
        };
        let cloned = event.clone();
        match cloned {
            ProgressEvent::RecipeStarted { name, total_nodes } => {
                assert_eq!(name, "build");
                assert_eq!(total_nodes, 5);
            }
            _ => panic!("unexpected variant"),
        }
    }

    // -----------------------------------------------------------------------
    // PlainRenderer tests
    // -----------------------------------------------------------------------

    fn make_renderer() -> (Arc<Mutex<Vec<u8>>>, PlainRenderer) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let color = ColorConfig::resolve(ColorMode::Never, false, false);
        let r = PlainRenderer::new(buf.clone(), color);
        (buf, r)
    }

    fn read_buf(buf: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn test_plain_renderer_output_line() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::OutputLine {
            recipe: "lib".into(),
            line: "gcc -c foo.c".into(),
            is_stderr: false,
        });
        assert_eq!(read_buf(&buf), "[lib] gcc -c foo.c\n");
    }

    #[test]
    fn test_plain_renderer_recipe_completed() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::RecipeCompleted {
            name: "lib".into(),
            elapsed: Duration::from_millis(1400),
            cached_nodes: 2,
            total_nodes: 6,
        });
        assert_eq!(read_buf(&buf), "[lib] done (1.4s, 2/6 cached)\n");
    }

    #[test]
    fn test_plain_renderer_recipe_completed_no_cache() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::RecipeCompleted {
            name: "app".into(),
            elapsed: Duration::from_millis(200),
            cached_nodes: 0,
            total_nodes: 4,
        });
        assert_eq!(read_buf(&buf), "[app] done (0.2s)\n");
    }

    #[test]
    fn test_plain_renderer_recipe_completed_all_cached() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::RecipeCompleted {
            name: "lib".into(),
            elapsed: Duration::from_millis(0),
            cached_nodes: 3,
            total_nodes: 3,
        });
        assert_eq!(read_buf(&buf), "[lib] done (0.0s, cached)\n");
    }

    #[test]
    fn test_plain_renderer_recipe_started_silent() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::RecipeStarted {
            name: "build".into(),
            total_nodes: 5,
        });
        assert_eq!(read_buf(&buf), "");
    }

    #[test]
    fn test_plain_renderer_node_cache_hit() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::NodeCacheHit {
            recipe: "lib".into(),
            node_name: "compile".into(),
        });
        assert_eq!(read_buf(&buf), "[lib] cached: compile\n");
    }

    #[test]
    fn test_plain_renderer_recipe_failed() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::RecipeFailed {
            name: "app".into(),
            elapsed: Duration::from_millis(500),
            completed_nodes: 3,
            total_nodes: 7,
        });
        assert_eq!(read_buf(&buf), "[app] FAILED (3/7 steps, 0.5s)\n");
    }

    #[test]
    fn test_plain_renderer_node_failed() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::NodeFailed {
            recipe: "lib".into(),
            node_name: "compile".into(),
            elapsed: Duration::from_millis(100),
            error: "command not found: gcc".into(),
        });
        assert_eq!(read_buf(&buf), "[lib] command not found: gcc\n");
    }

    #[test]
    fn test_plain_renderer_drain_clears_buffer() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::OutputLine {
            recipe: "lib".into(),
            line: "hello".into(),
            is_stderr: false,
        });
        let drained = r.drain();
        assert_eq!(String::from_utf8(drained).unwrap(), "[lib] hello\n");
        assert_eq!(read_buf(&buf), "");
    }

    #[test]
    fn test_plain_renderer_recipe_queued_silent() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::RecipeQueued {
            name: "lib".into(),
            total_nodes: 3,
        });
        assert_eq!(read_buf(&buf), "");
    }

    #[test]
    fn test_plain_renderer_finished_silent() {
        let (buf, mut r) = make_renderer();
        r.handle(ProgressEvent::Finished);
        assert_eq!(read_buf(&buf), "");
    }

    // -----------------------------------------------------------------------
    // TtyRenderer tests (state tracking only)
    // -----------------------------------------------------------------------

    fn make_tty_renderer() -> TtyRenderer {
        let color = ColorConfig::resolve(ColorMode::Never, false, false);
        TtyRenderer::new_for_test(color, 120)
    }

    #[test]
    fn test_tty_renderer_state_tracking() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 5,
        });
        assert_eq!(r.running_recipes, 1);
        assert!(r.recipes.contains_key("lib"));
        assert_eq!(r.recipes["lib"].total_nodes, 5);
        assert_eq!(r.recipes["lib"].completed_nodes, 0);

        r.update_state(&ProgressEvent::NodeStarted {
            recipe: "lib".into(),
            node_name: "compile a.c".into(),
        });
        assert_eq!(r.recipes["lib"].active_nodes.len(), 1);
        assert_eq!(r.recipes["lib"].active_nodes[0], "compile a.c");

        r.update_state(&ProgressEvent::NodeCompleted {
            recipe: "lib".into(),
            node_name: "compile a.c".into(),
            elapsed: Duration::from_millis(100),
        });
        assert_eq!(r.recipes["lib"].active_nodes.len(), 0);
        assert_eq!(r.recipes["lib"].completed_nodes, 1);
        assert_eq!(
            r.recipes["lib"].completed_node_names.get("compile a.c"),
            Some(&true)
        );
    }

    #[test]
    fn test_tty_renderer_cache_hit_tracking() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 3,
        });

        r.update_state(&ProgressEvent::NodeCacheHit {
            recipe: "lib".into(),
            node_name: "compile b.c".into(),
        });

        assert_eq!(r.recipes["lib"].completed_nodes, 1);
        assert_eq!(r.recipes["lib"].cached_nodes, 1);
        assert!(r.recipes["lib"].cached_node_names.contains("compile b.c"));
        assert_eq!(r.total_cached, 1);
    }

    #[test]
    fn test_tty_renderer_recipe_completion() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeQueued {
            name: "lib".into(),
            total_nodes: 2,
        });
        assert_eq!(r.total_recipes, 1);
        assert_eq!(r.waiting_recipes, 1);

        r.update_state(&ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 2,
        });
        assert_eq!(r.running_recipes, 1);
        assert_eq!(r.waiting_recipes, 0);

        r.update_state(&ProgressEvent::NodeStarted {
            recipe: "lib".into(),
            node_name: "compile a.c".into(),
        });
        r.update_state(&ProgressEvent::NodeCompleted {
            recipe: "lib".into(),
            node_name: "compile a.c".into(),
            elapsed: Duration::from_millis(50),
        });
        r.update_state(&ProgressEvent::NodeStarted {
            recipe: "lib".into(),
            node_name: "compile b.c".into(),
        });
        r.update_state(&ProgressEvent::NodeCompleted {
            recipe: "lib".into(),
            node_name: "compile b.c".into(),
            elapsed: Duration::from_millis(50),
        });

        r.update_state(&ProgressEvent::RecipeCompleted {
            name: "lib".into(),
            elapsed: Duration::from_millis(100),
            cached_nodes: 0,
            total_nodes: 2,
        });

        assert!(r.recipes["lib"].finished);
        assert!(!r.recipes["lib"].failed);
        assert_eq!(r.finished_recipes, 1);
        assert_eq!(r.running_recipes, 0);
        assert!(r.recipes["lib"].active_nodes.is_empty());
    }

    #[test]
    fn test_tty_renderer_node_failure_tracking() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 3,
        });

        r.update_state(&ProgressEvent::NodeStarted {
            recipe: "lib".into(),
            node_name: "compile bad.c".into(),
        });

        r.update_state(&ProgressEvent::NodeFailed {
            recipe: "lib".into(),
            node_name: "compile bad.c".into(),
            elapsed: Duration::from_millis(10),
            error: "gcc: error: bad.c: No such file".into(),
        });

        assert!(r.recipes["lib"].failed);
        assert!(r.recipes["lib"].active_nodes.is_empty());
        assert_eq!(r.recipes["lib"].completed_nodes, 1);
        assert_eq!(
            r.recipes["lib"].completed_node_names.get("compile bad.c"),
            Some(&false)
        );
        assert_eq!(r.recipes["lib"].error_output.len(), 1);
        assert_eq!(
            r.recipes["lib"].error_output[0],
            "gcc: error: bad.c: No such file"
        );
    }

    #[test]
    fn test_tty_renderer_node_skipped_tracking() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 2,
        });

        r.update_state(&ProgressEvent::NodeSkipped {
            recipe: "lib".into(),
            node_name: "link".into(),
        });

        assert_eq!(r.recipes["lib"].completed_nodes, 1);
        assert!(r.recipes["lib"].skipped_node_names.contains("link"));
    }

    #[test]
    fn test_tty_renderer_drain_returns_empty() {
        let r = make_tty_renderer();
        let drained = r.drain_buf();
        assert!(drained.is_empty());
    }

    #[test]
    fn test_tty_renderer_recipe_queued_then_started() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeQueued {
            name: "lib".into(),
            total_nodes: 3,
        });
        assert_eq!(r.total_recipes, 1);
        assert_eq!(r.waiting_recipes, 1);
        assert_eq!(r.running_recipes, 0);
        assert_eq!(r.total_nodes, 3);

        r.update_state(&ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 3,
        });
        assert_eq!(r.waiting_recipes, 0);
        assert_eq!(r.running_recipes, 1);
        assert_eq!(r.total_nodes, 3);
    }

    #[test]
    fn test_tty_renderer_recipe_order_preserved() {
        let mut r = make_tty_renderer();

        r.update_state(&ProgressEvent::RecipeQueued {
            name: "aaa".into(),
            total_nodes: 1,
        });
        r.update_state(&ProgressEvent::RecipeQueued {
            name: "bbb".into(),
            total_nodes: 1,
        });
        r.update_state(&ProgressEvent::RecipeQueued {
            name: "ccc".into(),
            total_nodes: 1,
        });

        assert_eq!(r.recipe_order, vec!["aaa", "bbb", "ccc"]);
    }

    #[test]
    fn test_tty_renderer_handle_updates_bars() {
        let mut r = make_tty_renderer();

        r.handle(ProgressEvent::RecipeQueued {
            name: "lib".into(),
            total_nodes: 2,
        });
        r.handle(ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 2,
        });
        r.handle(ProgressEvent::NodeStarted {
            recipe: "lib".into(),
            node_name: "compile a.c".into(),
        });
        r.handle(ProgressEvent::NodeCompleted {
            recipe: "lib".into(),
            node_name: "compile a.c".into(),
            elapsed: Duration::from_millis(50),
        });
        r.handle(ProgressEvent::RecipeCompleted {
            name: "lib".into(),
            elapsed: Duration::from_millis(100),
            cached_nodes: 0,
            total_nodes: 2,
        });
        r.handle(ProgressEvent::Finished);

        assert!(r.recipes["lib"].finished);
        assert_eq!(r.finished_recipes, 1);
    }
}
