//! Terminal init/teardown + event loop. See design spec §TUI, §Error Handling.

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::tty::IsTty;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Rect, Size};
use ratatui::Terminal;

use crate::frame::ViewFrame;
use crate::input;
use crate::render::canvas;
use crate::render::{self, RenderInputs};
use crate::state::AppState;
use crate::ViewerError;

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Result<Self, ViewerError> {
        enable_raw_mode().map_err(|e| ViewerError::TerminalInit(e.to_string()))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .map_err(|e| ViewerError::TerminalInit(e.to_string()))?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

pub fn run<F: ViewFrame>(frame: F) -> Result<(), ViewerError> {
    run_with_theme(frame, crate::theme::Theme::auto())
}

pub fn run_with_theme<F: ViewFrame>(
    mut frame: F,
    theme: crate::theme::Theme,
) -> Result<(), ViewerError> {
    if !io::stdout().is_tty() {
        return print_fallback(&frame);
    }

    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(backend).map_err(|e| ViewerError::TerminalInit(e.to_string()))?;

    let mut app = AppState::with_theme(frame.graph(), theme);

    loop {
        let layout = render::pick_layout(&app, frame.graph());
        let canvas_buf = canvas::render(&layout, &app, &frame);

        terminal
            .draw(|f| {
                let area = f.area();
                let buf = f.buffer_mut();
                render::draw(
                    area,
                    buf,
                    &mut app,
                    &frame,
                    RenderInputs { canvas: &canvas_buf, layout: &layout },
                );
            })
            .map_err(|e| ViewerError::TerminalInit(e.to_string()))?;

        if event::poll(Duration::from_millis(200))
            .map_err(|e| ViewerError::TerminalInit(e.to_string()))?
        {
            let evt = event::read().map_err(|e| ViewerError::TerminalInit(e.to_string()))?;
            let size = terminal.size().unwrap_or(Size::ZERO);
            let size_rect = Rect::new(0, 0, size.width, size.height);
            input::handle(&mut app, &layout, &frame, &evt, size_rect);
            if app.follow {
                let pane = graph_pane_rect_from_terminal(size_rect);
                app.recenter(&layout, pane);
            }
        }

        // Future: drain frame.poll_event() here for live mode.
        let _ = &mut frame; // silence "unused mut" until poll_event is used.

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn graph_pane_rect_from_terminal(t: Rect) -> Rect {
    let body_h = t.height.saturating_sub(2);
    let detail_h = 6.min(body_h);
    let graph_h = body_h.saturating_sub(detail_h);
    Rect::new(28, 1, t.width.saturating_sub(28), graph_h)
}

fn print_fallback<F: ViewFrame>(frame: &F) -> Result<(), ViewerError> {
    let g = frame.graph();
    println!("cook dag (non-TTY fallback) — target {}", g.target);
    for (wi, wave) in g.waves.iter().enumerate() {
        println!("Wave {}", wi);
        for recipe in &wave.recipes {
            println!("  {}", recipe);
            for n in &wave.nodes {
                if n.kind == "unit" && n.recipe.as_deref() == Some(recipe) {
                    println!("    {} ({})", n.label, n.id);
                }
            }
        }
    }
    Ok(())
}
