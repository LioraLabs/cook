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
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    input::handle(&mut app, &layout, &frame, &key('j'), term); // down to recipe
    input::handle(&mut app, &layout, &frame, &key('l'), term); // expand recipe
    input::handle(&mut app, &layout, &frame, &key('j'), term); // down into first unit

    assert_eq!(app.selection, Selection::unit(0, 0, 0));
}

#[test]
fn capital_l_pans_camera_and_disables_follow() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    let cam_before = app.camera_x;
    input::handle(&mut app, &layout, &frame, &keymod('L', KeyModifiers::SHIFT), term);
    assert!(app.camera_x >= cam_before);
    assert!(!app.follow);
}

#[test]
fn slash_then_typed_query_then_enter_jumps_to_match() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    input::handle(&mut app, &layout, &frame, &key('/'), term);
    assert_eq!(app.mode, Mode::Search);
    for ch in "libfoo".chars() {
        input::handle(&mut app, &layout, &frame, &key(ch), term);
    }
    input::handle(&mut app, &layout, &frame, &special(KeyCode::Enter), term);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.selection.node_id(&app.tree), Some("unit:cpp.link:0"));
}

#[test]
fn q_quits() {
    let g = fixtures::three_wave_dag();
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let mut app = AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);
    input::handle(&mut app, &layout, &frame, &key('q'), term);
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
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    let initial = app.density;
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('m'), term);
    assert_ne!(app.density, initial);

    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('m'), term);
    assert_eq!(app.density, initial, "two presses complete the Full↔Compact cycle");
}

#[test]
fn p_toggles_pin_on_selected_unit() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    // Walk into the first unit (Wave 0 → cpp.compile → unit 0).
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term); // wave 0 row → first recipe
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('l'), term); // expand recipe
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term); // descend to unit 0

    // Pin
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('p'), term);
    assert_eq!(app.pins.slot_of("unit:cpp.compile:0"), Some(0));

    // Unpin
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('p'), term);
    assert_eq!(app.pins.slot_of("unit:cpp.compile:0"), None);
}

#[test]
fn p_emits_full_message_when_slots_exhausted() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    // Synthesise 9 pinned slots directly.
    for i in 0..9 {
        app.pins.pin(&format!("synth:{i}"));
    }
    assert!(app.pins.is_full());

    // Walk to a unit and try to pin.
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('l'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('p'), term);

    assert_eq!(
        app.last_pin_message,
        Some(cook_dag_viewer::state::PinMsg::Full),
    );
}

#[test]
fn capital_p_pins_every_unit_in_selected_recipe() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('l'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term);

    cook_dag_viewer::input::handle(
        &mut app,
        &layout,
        &frame,
        &keymod('P', KeyModifiers::SHIFT),
        term,
    );

    // three_wave_dag's cpp.compile recipe has exactly one unit.
    assert_eq!(app.pins.iter().count(), 1);
    assert!(app.pins.slot_of("unit:cpp.compile:0").is_some());
}

#[test]
fn capital_p_unpins_when_all_recipe_units_pinned() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('l'), term);
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('j'), term);

    // First press pins the one unit in the recipe.
    cook_dag_viewer::input::handle(
        &mut app,
        &layout,
        &frame,
        &keymod('P', KeyModifiers::SHIFT),
        term,
    );
    assert_eq!(app.pins.iter().count(), 1);

    // Second press unpins it.
    cook_dag_viewer::input::handle(
        &mut app,
        &layout,
        &frame,
        &keymod('P', KeyModifiers::SHIFT),
        term,
    );
    assert_eq!(app.pins.iter().count(), 0);
}

#[test]
fn capital_p_on_file_node_emits_on_file_message() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    // Walk to a file node (search for "bar.cpp").
    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('/'), term);
    for ch in "bar.cpp".chars() {
        cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key(ch), term);
    }
    cook_dag_viewer::input::handle(
        &mut app,
        &layout,
        &frame,
        &special(KeyCode::Enter),
        term,
    );

    cook_dag_viewer::input::handle(
        &mut app,
        &layout,
        &frame,
        &keymod('P', KeyModifiers::SHIFT),
        term,
    );
    assert_eq!(
        app.last_pin_message,
        Some(cook_dag_viewer::state::PinMsg::OnFile),
    );
    assert!(app.pins.is_empty());
}

#[test]
fn capital_x_clears_all_pins() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    app.pins.pin("synth:1");
    app.pins.pin("synth:2");

    cook_dag_viewer::input::handle(
        &mut app,
        &layout,
        &frame,
        &keymod('X', KeyModifiers::SHIFT),
        term,
    );
    assert!(app.pins.is_empty());
    assert_eq!(
        app.last_pin_message,
        Some(cook_dag_viewer::state::PinMsg::ClearedAll(2)),
    );
}

#[test]
fn digit_jumps_selection_to_pin_slot() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    // Pin two nodes by ID.
    app.pins.pin("unit:cpp.compile:0");
    app.pins.pin("unit:cpp.link:0");

    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('2'), term);
    assert_eq!(
        app.selection.node_id(&app.tree),
        Some("unit:cpp.link:0"),
        "pressing 2 should jump selection to the slot-1 pin (1-indexed)",
    );
}

#[test]
fn digit_emits_empty_slot_message_when_slot_unused() {
    let g = fixtures::three_wave_dag();
    let layout = cook_dag_viewer::render::layout::compute(
        &g,
        cook_dag_viewer::render::layout::LayoutDims::FULL,
    );
    let mut app = cook_dag_viewer::state::AppState::new(&g);
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);

    cook_dag_viewer::input::handle(&mut app, &layout, &frame, &key('5'), term);
    assert_eq!(
        app.last_pin_message,
        Some(cook_dag_viewer::state::PinMsg::EmptySlot(4)),
    );
}

#[test]
fn compact_mode_layout_filters_to_one_hop_for_unit_selection() {
    use cook_dag_viewer::state::{DensityMode, Selection};
    let g = fixtures::three_wave_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Compact;

    // Walk to the first unit: cpp.compile → unit 0.
    let frame = cook_dag_viewer::SnapshotFrame::new(g.clone());
    let term = Rect::new(0, 0, 120, 40);
    let layout_compact = layout::compute(&g, layout::LayoutDims::COMPACT);
    cook_dag_viewer::input::handle(&mut app, &layout_compact, &frame, &key('j'), term);
    cook_dag_viewer::input::handle(&mut app, &layout_compact, &frame, &key('l'), term);
    cook_dag_viewer::input::handle(&mut app, &layout_compact, &frame, &key('j'), term);
    assert_eq!(app.selection, Selection::unit(0, 0, 0));

    let layout = cook_dag_viewer::render::pick_layout(&app, &g);
    let ids: std::collections::BTreeSet<&str> =
        layout.nodes.iter().map(|n| n.id.as_str()).collect();
    // Selected unit must be present.
    assert!(ids.contains("unit:cpp.compile:0"));
    // The unit's declared inputs must be present (file:bar.cpp is a direct input).
    assert!(ids.iter().any(|id| id.starts_with("file:")));
    // unit:cpp.link:0 is a direct 1-hop downstream neighbor via inter-wave edge,
    // so it IS included in the ego graph.
    assert!(
        ids.contains("unit:cpp.link:0"),
        "compact focus should include direct 1-hop downstream neighbors, got {ids:?}",
    );
    // Compact mode must not include nodes from unrelated waves that are not
    // reachable within 1 hop; verify by checking the layout is a proper subgraph
    // (wave-level focus: selecting wave 0 drops wave 1 nodes — see spec §3.1).
    let mut wave_app = AppState::new(&g);
    wave_app.density = DensityMode::Compact;
    // Default selection is wave_only(0); focus_subgraph returns only wave 0 nodes.
    assert_eq!(wave_app.selection, Selection::wave_only(0));
    let wave_layout = cook_dag_viewer::render::pick_layout(&wave_app, &g);
    let wave_ids: std::collections::BTreeSet<&str> =
        wave_layout.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(wave_ids.contains("unit:cpp.compile:0"), "wave 0 compile unit must be present");
    assert!(
        !wave_ids.contains("unit:cpp.link:0"),
        "wave-level compact focus must not expand across inter-wave edges (spec §3.1), got {wave_ids:?}",
    );
}

#[test]
fn full_mode_layout_still_renders_entire_graph() {
    use cook_dag_viewer::state::DensityMode;
    let g = fixtures::three_wave_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Full;
    let layout = cook_dag_viewer::render::pick_layout(&app, &g);
    let ids: std::collections::BTreeSet<&str> =
        layout.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains("unit:cpp.compile:0"));
    assert!(ids.contains("unit:cpp.link:0"), "Full mode regression: link unit dropped");
}
