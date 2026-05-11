//! Event loop: draw, read key, mutate state, repeat.

use std::io::stdout;
use std::path::PathBuf;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
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
        terminal
            .draw(|f| draw_frame(f, &state, &theme))
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
            }
        }
    }
    Ok(())
}

fn draw_frame(f: &mut ratatui::Frame<'_>, state: &UiState, theme: &Theme) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    render::header::draw(f, chunks[0], state, theme);

    if area.width < 60 {
        match state.focus {
            Focus::Tree => render::tree::draw(f, chunks[1], state, theme),
            Focus::Output => render::output::draw(f, chunks[1], state, theme),
        }
    } else {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(chunks[1]);
        render::tree::draw(f, panes[0], state, theme);
        render::output::draw(f, panes[1], state, theme);
    }

    // Picker rendered later (Task 15 will populate it).
    if state.show_help {
        render::help::draw(f, area, &state.diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cook_progress::event::{NodeId, NodeKind, RecipeId};
    use cook_progress::log_reader::{NodeView, RecipeView};
    use cook_progress::model::{NodeStatus, Status};
    use ratatui::backend::TestBackend;
    use std::collections::BTreeMap;

    fn one_failed_build() -> BuildView {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            NodeId::new(0),
            NodeView {
                name: "lvm.c".into(),
                status: NodeStatus::Failed,
                kind: NodeKind::Cooked,
                started_at: None,
                ended_at: None,
                elapsed_ms: Some(1100),
                skip_reason: None,
                lines: vec![cook_progress::log_reader::LogLine {
                    stream: cook_progress::event::Stream::Stderr,
                    ts: None,
                    text: "error: undeclared 'foo'".into(),
                }],
            },
        );
        let mut recipes = BTreeMap::new();
        recipes.insert(
            RecipeId::new(0),
            RecipeView {
                name: "vm".into(),
                status: Status::Failed,
                nodes,
            },
        );
        BuildView {
            build_id: "2026-05-10-abc".into(),
            started_at: "2026-05-10T10:00:00Z".into(),
            ended_at: Some("2026-05-10T10:00:12Z".into()),
            exit_code: Some(1),
            recipes,
        }
    }

    #[test]
    fn renders_one_frame_with_failed_node_visible() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = UiState::new(one_failed_build(), LoadDiagnostics::default());
        let frame = terminal
            .draw(|f| draw_frame(f, &state, &Theme::default()))
            .unwrap();
        let content: String = frame
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("lvm.c"), "tree pane should show node name");
        assert!(
            content.contains("error: undeclared"),
            "output pane should show log line"
        );
        assert!(
            content.contains("2026-05-10-abc"),
            "header should show build id"
        );
    }
}
