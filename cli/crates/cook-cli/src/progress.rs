//! Selects and spawns the new cook-progress Driver.

use cook_progress::{Driver, InlineRenderer, JsonWriter, LogConfig, LogStore, PlainRenderer, Renderer};
use indicatif::ProgressDrawTarget;

#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Auto,
    Plain,
    Json,
}

impl OutputMode {
    pub fn from_cli(cli: &crate::cli::Cli) -> Self {
        if cli.output == "json" {
            return Self::Json;
        }
        if cli.output == "plain" || cli.no_ui {
            return Self::Plain;
        }
        Self::Auto
    }
}

/// Spawn the cook-progress Driver reading from `rx`. Returns a JoinHandle that
/// yields the build success flag when the driver's run loop exits.
pub fn spawn_new_renderer(
    cli: &crate::cli::Cli,
    project_root: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<cook_progress::ProgressEvent>,
) -> std::thread::JoinHandle<bool> {
    let mode = OutputMode::from_cli(cli);
    std::thread::spawn(move || {
        let log_store = LogStore::open(&project_root, LogConfig::default()).ok();

        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let ci = std::env::var("CI").ok().is_some();
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);

        let renderer: Box<dyn Renderer> = match mode {
            OutputMode::Json => Box::new(JsonWriter::new(std::io::stderr())),
            OutputMode::Plain => Box::new(PlainRenderer::new(std::io::stderr())),
            OutputMode::Auto if !is_tty || ci || dumb => {
                Box::new(PlainRenderer::new(std::io::stderr()))
            }
            OutputMode::Auto => Box::new(InlineRenderer::new(ProgressDrawTarget::stderr())),
        };

        let mut driver = Driver::new(renderer, log_store);
        driver.run(rx).unwrap_or(false)
    })
}
