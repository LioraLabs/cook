//! Keyboard + mouse event handling. Filled in across Tasks 10–14.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::render::layout::Layout;
use crate::state::{AppState, Mode};

pub fn handle<F: crate::frame::ViewFrame>(
    app: &mut AppState,
    layout: &Layout,
    frame: &F,
    event: &Event,
    size: Rect,
) {
    let pane = graph_pane_rect(size);
    match event {
        Event::Key(key) => match app.mode {
            Mode::Normal => normal_key(app, key, layout, pane, frame),
            Mode::EdgePicker => picker_key(app, key),
            Mode::Search => search_key(app, key),
            Mode::Help | Mode::DetailOverlay => overlay_key(app, key),
        },
        Event::Mouse(m) => mouse(app, layout, m, pane),
        _ => {}
    }
}

fn mouse(app: &mut AppState, layout: &Layout, m: &MouseEvent, pane: Rect) {
    let in_pane = m.column >= pane.x
        && m.column < pane.x + pane.width
        && m.row >= pane.y
        && m.row < pane.y + pane.height;
    if !in_pane {
        return;
    }
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let cx = (m.column - pane.x) as i32 + app.camera_x;
            let cy = (m.row - pane.y) as i32 + app.camera_y;
            for node in &layout.nodes {
                let in_box = cx >= node.x as i32
                    && cx < node.x as i32 + node.w as i32
                    && cy >= node.y as i32
                    && cy < node.y as i32 + node.h as i32;
                if in_box {
                    app.jump_to_node(&node.id);
                    return;
                }
            }
        }
        MouseEventKind::ScrollUp if m.modifiers.contains(KeyModifiers::SHIFT) => {
            app.pan_camera(-(pane.width as i32) / 8, 0, layout, pane);
        }
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::SHIFT) => {
            app.pan_camera((pane.width as i32) / 8, 0, layout, pane);
        }
        MouseEventKind::ScrollUp => app.pan_camera(0, -(pane.height as i32) / 8, layout, pane),
        MouseEventKind::ScrollDown => app.pan_camera(0, (pane.height as i32) / 8, layout, pane),
        _ => {}
    }
}

fn normal_key<F: crate::frame::ViewFrame>(
    app: &mut AppState,
    key: &KeyEvent,
    layout: &Layout,
    pane: Rect,
    frame: &F,
) {
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
        (KeyCode::Char('m'), KeyModifiers::NONE) => {
            app.density = app.density.next();
        }
        (KeyCode::Char('s'), KeyModifiers::NONE) => {
            if matches!(app.density, crate::state::DensityMode::Flow) {
                app.glyph = app.glyph.next();
            }
        }
        (KeyCode::Char('R'), _) => {
            if matches!(app.density, crate::state::DensityMode::Flow) {
                app.radial = !app.radial;
            }
        }
        (KeyCode::Char('p'), KeyModifiers::NONE) => app.toggle_pin_selected(),
        (KeyCode::Char('P'), _) => app.bulk_pin_recipe(frame.graph()),
        (KeyCode::Char('X'), _) => app.clear_all_pins(),
        (KeyCode::Char(c), KeyModifiers::NONE)
            if c.is_ascii_digit() && c != '0' =>
        {
            let slot = (c as u8 - b'1') as usize;
            app.jump_to_pin_slot(slot);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{NodeData, WaveData, WaveDagData};
    use crate::frame::SnapshotFrame;
    use crate::render::layout;
    use crate::state::{DensityMode, GlyphStyle};

    fn dag() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "t".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![NodeData {
                    id: "unit:a:0".into(),
                    kind: "unit".into(),
                    label: "a0".into(),
                    recipe: Some("a".into()),
                    command: Some("c".into()),
                    output: None,
                    cached: Some(true),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                }],
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        }
    }

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    #[test]
    fn s_cycles_glyph_in_flow_mode() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.density = DensityMode::Flow;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FLOW);
        let size = Rect::new(0, 0, 80, 24);
        handle(&mut app, &layout, &frame, &key('s'), size);
        assert_eq!(app.glyph, GlyphStyle::Diamond);
    }

    #[test]
    fn s_is_noop_outside_flow_mode() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.density = DensityMode::Full;
        let original = app.glyph;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FULL);
        let size = Rect::new(0, 0, 80, 24);
        handle(&mut app, &layout, &frame, &key('s'), size);
        assert_eq!(app.glyph, original);
    }

    #[test]
    fn shift_r_toggles_radial_in_flow_mode() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.density = DensityMode::Flow;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FLOW);
        let size = Rect::new(0, 0, 80, 24);
        handle(&mut app, &layout, &frame, &key('R'), size);
        assert!(app.radial);
        handle(&mut app, &layout, &frame, &key('R'), size);
        assert!(!app.radial);
    }
}
