//! Keyboard + mouse event handling. Filled in across Tasks 10–14.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::render::layout::Layout;
use crate::state::{AppState, Mode};

pub fn handle(app: &mut AppState, layout: &Layout, event: &Event, size: Rect) {
    let pane = graph_pane_rect(size);
    let Event::Key(key) = event else { return };
    match app.mode {
        Mode::Normal => normal_key(app, key, layout, pane),
        Mode::EdgePicker => picker_key(app, key),
        Mode::Search => search_key(app, key),
        Mode::Help | Mode::DetailOverlay => overlay_key(app, key),
    }
}

fn normal_key(app: &mut AppState, key: &KeyEvent, layout: &Layout, pane: Rect) {
    let pane_w = pane.width;
    let pane_h = pane.height;
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
        (KeyCode::Char(']'), KeyModifiers::NONE) => {
            app.open_edge_picker_for_selection(crate::state::PickerDir::Downstream);
        }
        (KeyCode::Char('['), KeyModifiers::NONE) => {
            app.open_edge_picker_for_selection(crate::state::PickerDir::Upstream);
        }
        (KeyCode::Char('H'), _) => app.pan_camera(-(pane_w as i32) / 2, 0, layout, pane),
        (KeyCode::Char('L'), _) => app.pan_camera((pane_w as i32) / 2, 0, layout, pane),
        (KeyCode::Char('J'), _) => app.pan_camera(0, (pane_h as i32) / 2, layout, pane),
        (KeyCode::Char('K'), _) => app.pan_camera(0, -(pane_h as i32) / 2, layout, pane),
        (KeyCode::Char('c'), KeyModifiers::NONE) => app.recenter(layout, pane),
        (KeyCode::Char('a'), KeyModifiers::NONE) => app.auto_fit(layout, pane),
        (KeyCode::Char('/'), _) => app.mode = Mode::Search,
        (KeyCode::Char('?'), _) => app.mode = Mode::Help,
        (KeyCode::Char('v'), KeyModifiers::NONE) => app.mode = Mode::DetailOverlay,
        (KeyCode::Char('n'), KeyModifiers::NONE) => {
            let n = app.search.matches.len();
            if n > 0 {
                app.search.cursor = (app.search.cursor + 1) % n;
                let id = app.search.matches[app.search.cursor].clone();
                app.jump_to_node(&id);
            }
        }
        (KeyCode::Char('N'), _) => {
            let n = app.search.matches.len();
            if n > 0 {
                app.search.cursor = (app.search.cursor + n - 1) % n;
                let id = app.search.matches[app.search.cursor].clone();
                app.jump_to_node(&id);
            }
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) => {
            // refresh — snapshot mode is a no-op; documented in spec.
        }
        _ => {}
    }
}

fn picker_key(app: &mut AppState, key: &KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.edge_picker = crate::state::EdgePicker::default();
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let n = app.edge_picker.candidates.len();
            if n > 0 {
                app.edge_picker.cursor = (app.edge_picker.cursor + 1) % n;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let n = app.edge_picker.candidates.len();
            if n > 0 {
                app.edge_picker.cursor = (app.edge_picker.cursor + n - 1) % n;
            }
        }
        KeyCode::Enter => {
            let id = app.edge_picker.candidates[app.edge_picker.cursor].clone();
            app.jump_to_node(&id);
            app.mode = Mode::Normal;
            app.edge_picker = crate::state::EdgePicker::default();
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let idx = c.to_digit(10).unwrap() as usize;
            if idx >= 1 && idx <= app.edge_picker.candidates.len() {
                let id = app.edge_picker.candidates[idx - 1].clone();
                app.jump_to_node(&id);
                app.mode = Mode::Normal;
                app.edge_picker = crate::state::EdgePicker::default();
            }
        }
        _ => {}
    }
}

fn search_key(app: &mut AppState, key: &KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.search = Default::default();
        }
        KeyCode::Enter => {
            if let Some(id) = app.search.matches.first().cloned() {
                app.jump_to_node(&id);
            }
            app.mode = Mode::Normal;
        }
        KeyCode::Backspace => {
            app.search.query.pop();
            let g = app.graph.clone();
            app.search.update(&g);
        }
        KeyCode::Char(c) => {
            app.search.query.push(c);
            let g = app.graph.clone();
            app.search.update(&g);
        }
        _ => {}
    }
}

fn overlay_key(app: &mut AppState, key: &KeyEvent) {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        app.mode = Mode::Normal;
    }
}

fn graph_pane_rect(terminal: Rect) -> Rect {
    let body_h = terminal.height.saturating_sub(2);
    let detail_h = 6.min(body_h);
    let graph_h = body_h.saturating_sub(detail_h);
    Rect::new(28, 1, terminal.width.saturating_sub(28), graph_h)
}
