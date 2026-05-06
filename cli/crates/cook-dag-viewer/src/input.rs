//! Keyboard + mouse event handling. Filled in across Tasks 10–12, 14.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::render::layout::Layout;
use crate::state::{AppState, Mode};

pub fn handle(app: &mut AppState, _layout: &Layout, event: &Event, _size: Rect) {
    let Event::Key(key) = event else { return };
    match app.mode {
        Mode::Normal => normal_key(app, key),
        _ => {} // overlay modes handled in later tasks.
    }
}

fn normal_key(app: &mut AppState, key: &KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => app.move_cursor(false),
        (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => app.move_cursor(true),
        (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
            app.collapse_or_step_out();
        }
        (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
            app.expand_or_step_in();
        }
        (KeyCode::Tab, _) => {
            app.expand_or_step_in();
            app.move_cursor(false);
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => app.jump_first(),
        (KeyCode::Char('G'), _) => app.jump_last(),
        _ => {}
    }
}
