//! Event loop: draw, read key, mutate state, repeat.

use std::io::stdout;
use std::path::PathBuf;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::tty::IsTty;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Terminal;

use cook_progress::log_reader::{self, BuildView, LoadDiagnostics};

use crate::error::ViewerError;
use crate::input::{self, Action};
use crate::render;
use crate::state::{Focus, UiState};
use crate::theme::Theme;

pub fn run(view: BuildView, diag: LoadDiagnostics, theme: Theme) -> Result<(), ViewerError> {
    if !std::io::stdout().is_tty() {
        return print_logs_fallback(&view);
    }

    enable_raw_mode().map_err(|e| ViewerError::TerminalInit(e.to_string()))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| ViewerError::TerminalInit(e.to_string()))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal =
        Terminal::new(backend).map_err(|e| ViewerError::TerminalInit(e.to_string()))?;
    let result = run_with_backend(view, diag, theme, &mut terminal);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

/// Plain-text rendering of a build's logs for headless (non-TTY) contexts,
/// e.g. `cook logs | less` or output piped to a file in CI.
fn print_logs_fallback(view: &BuildView) -> Result<(), ViewerError> {
    let stdout = std::io::stdout();
    write_logs_fallback(view, &mut stdout.lock())
        .map_err(|e| ViewerError::TerminalInit(e.to_string()))
}

fn write_logs_fallback<W: std::io::Write>(
    view: &BuildView,
    out: &mut W,
) -> std::io::Result<()> {
    writeln!(out, "build {} (exit {:?})", view.build_id, view.exit_code)?;
    for recipe in view.recipes.values() {
        writeln!(out, "  {} [{:?}]", recipe.name, recipe.status)?;
        for node in recipe.nodes.values() {
            let elapsed = node
                .elapsed_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| "-".to_string());
            writeln!(out, "    {} [{:?}] {}", node.name, node.status, elapsed)?;
            for line in &node.lines {
                writeln!(out, "      {}", line.text)?;
            }
        }
    }
    Ok(())
}

pub fn run_with_backend<B: Backend>(
    view: BuildView,
    diag: LoadDiagnostics,
    theme: Theme,
    terminal: &mut Terminal<B>,
) -> Result<(), ViewerError> {
    let mut state = UiState::new(view, diag);
    let logs_root: Option<PathBuf> =
        std::env::current_dir().ok().map(|d| d.join(".cook").join("logs"));

    loop {
        if let Some(p) = state.picker.as_mut() {
            if p.builds.is_empty() {
                if let Some(root) = &logs_root {
                    if let Ok(list) = log_reader::list_builds(root) {
                        p.builds = list;
                    }
                }
            }
        }
        terminal
            .draw(|f| draw_frame(f, &mut state, &theme))
            .map_err(|e| ViewerError::Layout(e.to_string()))?;
        let evt = event::read().map_err(|e| ViewerError::Layout(e.to_string()))?;
        if let Event::Key(key) = evt {
            match input::handle_key(&mut state, key) {
                Action::Quit => break,
                Action::Continue => {}
                Action::Reload => {
                    if let Some(root) = &logs_root {
                        let build_dir = root.join(&state.view.build_id);
                        if let Ok((view, diag)) = log_reader::load(&build_dir) {
                            state = UiState::new(view, diag);
                        }
                    }
                }
                Action::YankSelectedLog => {
                    if let Some((rid, nid)) = state.selected_node() {
                        if let Some(recipe) = state.view.recipes.get(&rid) {
                            if let Some(node) = recipe.nodes.get(&nid) {
                                let text = node.lines.iter()
                                    .map(|l| l.text.clone())
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                use base64::Engine;
                                let payload = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
                                // OSC 52: ESC ] 52 ; c ; <base64> BEL
                                let _ = std::io::Write::write_all(
                                    &mut std::io::stdout(),
                                    format!("\x1b]52;c;{}\x07", payload).as_bytes(),
                                );
                            }
                        }
                    }
                }
                Action::SwitchBuild(target_id) => {
                    if let Some(root) = &logs_root {
                        let build_dir = root.join(&target_id);
                        if let Ok((view, diag)) = log_reader::load(&build_dir) {
                            // Preserve a few UI prefs across builds per spec §10.
                            let prev_filter = state.filter;
                            let prev_show_ts = state.show_timestamps;
                            let prev_soft_wrap = state.soft_wrap;
                            state = UiState::new(view, diag);
                            state.filter = prev_filter;
                            state.show_timestamps = prev_show_ts;
                            state.soft_wrap = prev_soft_wrap;
                            state.rebuild_flat();
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn draw_frame(f: &mut ratatui::Frame<'_>, state: &mut UiState, theme: &Theme) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    render::header::draw(f, chunks[0], state, theme);

    if area.width < 60 {
        state.ensure_tree_visible(chunks[1].height as usize);
        match state.focus {
            Focus::Tree => render::tree::draw(f, chunks[1], state, theme),
            Focus::Output => render::output::draw(f, chunks[1], state, theme),
        }
    } else {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(chunks[1]);
        state.ensure_tree_visible(panes[0].height as usize);
        render::tree::draw(f, panes[0], state, theme);
        render::output::draw(f, panes[1], state, theme);
    }

    if let Some(p) = state.picker.as_ref() {
        render::picker::draw(f, area, &p.builds, p.cursor);
    }
    if state.show_help {
        render::help::draw(f, area, &state.diagnostics);
    }
}

#[cfg(test)]
#[path = "tests/tui_tests.rs"]
mod tests;
