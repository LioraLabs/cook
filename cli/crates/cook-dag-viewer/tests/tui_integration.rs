//! End-to-end-ish: drive AppState through scripted key events.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::layout::Rect;

use cook_dag_viewer::input;
use cook_dag_viewer::render::layout;
use cook_dag_viewer::state::{AppState, Mode, Selection};

mod fixtures;

fn key(c: char) -> Event {
    Event::Key(KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

fn keymod(c: char, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent {
        code: KeyCode::Char(c),
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

fn special(code: KeyCode) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

#[test]
fn jjl_walks_into_first_unit() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let term = Rect::new(0, 0, 120, 40);

    input::handle(&mut app, &layout, &key('j'), term); // down to recipe
    input::handle(&mut app, &layout, &key('l'), term); // expand recipe
    input::handle(&mut app, &layout, &key('j'), term); // down into first unit

    assert_eq!(app.selection, Selection { wave: 0, recipe: Some(0), unit: Some(0) });
}

#[test]
fn capital_l_pans_camera_and_disables_follow() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let term = Rect::new(0, 0, 120, 40);

    let cam_before = app.camera_x;
    input::handle(&mut app, &layout, &keymod('L', KeyModifiers::SHIFT), term);
    assert!(app.camera_x >= cam_before);
    assert!(!app.follow);
}

#[test]
fn slash_then_typed_query_then_enter_jumps_to_match() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let term = Rect::new(0, 0, 120, 40);

    input::handle(&mut app, &layout, &key('/'), term);
    assert_eq!(app.mode, Mode::Search);
    for ch in "libfoo".chars() {
        input::handle(&mut app, &layout, &key(ch), term);
    }
    input::handle(&mut app, &layout, &special(KeyCode::Enter), term);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.selection.node_id(&app.tree), Some("unit:cpp.link:0"));
}

#[test]
fn q_quits() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let term = Rect::new(0, 0, 120, 40);
    input::handle(&mut app, &layout, &key('q'), term);
    assert!(app.should_quit);
}

#[test]
fn m_cycles_density_mode() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let term = Rect::new(0, 0, 120, 40);

    let initial = app.density;
    cook_dag_viewer::input::handle(&mut app, &layout, &key('m'), term);
    assert_ne!(app.density, initial);
    let after_one = app.density;

    cook_dag_viewer::input::handle(&mut app, &layout, &key('m'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &key('m'), term);
    assert_eq!(app.density, initial, "three presses complete the cycle");
    assert_ne!(app.density, after_one);
}
