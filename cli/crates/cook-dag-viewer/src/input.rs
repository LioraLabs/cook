//! Keyboard + mouse event handling. Filled in by Tasks 10–12, 14.

use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::layout::Rect;

use crate::render::layout::Layout;
use crate::state::AppState;

pub fn handle(app: &mut AppState, _layout: &Layout, event: &Event, _size: Rect) {
    if let Event::Key(KeyEvent { code: KeyCode::Char('q'), .. }) = event {
        app.should_quit = true;
    }
    // Filled in by later tasks.
}
