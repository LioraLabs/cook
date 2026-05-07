//! Arrow-key bindings: parity with hjkl in normal mode, ctrl+arrows pan camera.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::layout::Rect;

use cook_dag_viewer::input;
use cook_dag_viewer::render::layout;
use cook_dag_viewer::state::AppState;

mod fixtures;

fn special(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE })
}

fn key(c: char) -> Event {
    Event::Key(KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

#[test]
fn down_arrow_moves_cursor_like_j() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let term = Rect::new(0, 0, 120, 40);

    let mut a = AppState::new(&g);
    let mut b = AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());

    input::handle(&mut a, &layout, &frame, &key('j'), term);
    input::handle(&mut b, &layout, &frame, &special(KeyCode::Down, KeyModifiers::NONE), term);

    assert_eq!(a.selection, b.selection);
}

#[test]
fn ctrl_right_pans_camera_and_does_not_move_cursor() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let term = Rect::new(0, 0, 120, 40);

    let mut app = AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());

    let cam_before = app.camera_x;
    let sel_before = app.selection;
    input::handle(
        &mut app,
        &layout,
        &frame,
        &special(KeyCode::Right, KeyModifiers::CONTROL),
        term,
    );
    assert!(app.camera_x >= cam_before);
    assert!(!app.follow);
    assert_eq!(app.selection, sel_before);
}
