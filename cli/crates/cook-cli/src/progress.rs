//! Selects and spawns the cook-progress Driver.

use cook_progress::{
    Driver, EventWriterOptions, InlineOptions, InlineRenderer, JsonWriter,
    LogConfig, LogStore, PlainRenderer, Renderer, StatusLineOptions,
};

#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Auto,
    Plain,
    Json,
}

impl OutputMode {
    pub fn from_globals(globals: &crate::cli::Globals) -> Self {
        match globals.output.as_str() {
            "json" => Self::Json,
            "plain" => Self::Plain,
            _ => Self::Auto,
        }
    }
}

/// Spawn the cook-progress Driver reading from `rx`. Returns a JoinHandle that
/// yields the build success flag when the driver's run loop exits.
pub fn spawn_new_renderer(
    globals: &crate::cli::Globals,
    project_root: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<cook_progress::ProgressEvent>,
) -> std::thread::JoinHandle<bool> {
    let mode = OutputMode::from_globals(globals);
    let quiet = globals.quiet;
    let verbose = globals.verbose;
    let cli_color = globals.color.clone();
    let no_progress = std::env::var("NO_PROGRESS").is_ok();

    std::thread::spawn(move || {
        let log_store = LogStore::open(&project_root, LogConfig::default()).ok();

        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let ci = std::env::var("CI").ok().is_some();
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
        let no_color_env = std::env::var("NO_COLOR").is_ok();
        let colored = match cli_color.as_str() {
            "always" => !no_color_env,
            "never" => false,
            _ => is_tty && !no_color_env,
        };

        let renderer: Box<dyn Renderer> = match mode {
            OutputMode::Json => Box::new(JsonWriter::new(std::io::stderr())),
            OutputMode::Plain => Box::new(PlainRenderer::new(std::io::stderr())),
            OutputMode::Auto if !is_tty || ci || dumb => {
                Box::new(PlainRenderer::new(std::io::stderr()))
            }
            OutputMode::Auto => {
                let opts = InlineOptions {
                    event: EventWriterOptions {
                        colored,
                        quiet,
                        verbose,
                        cached_inline_threshold: 8,
                    },
                    status: StatusLineOptions {
                        colored,
                        min_nodes: 5,
                    },
                    status_enabled: !no_progress && !quiet,
                };
                Box::new(InlineRenderer::new(opts))
            }
        };

        let mut driver = Driver::new(renderer, log_store);
        driver.run(rx).unwrap_or(false)
    })
}
