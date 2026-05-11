//! Translate `crossterm::KeyEvent` into state mutations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::state::{Focus, UiState};

pub enum Action { Continue, Quit, Reload, SwitchBuild(String) }

pub fn handle_key(state: &mut UiState, key: KeyEvent) -> Action {
    if state.picker.is_some() {
        return handle_picker(state, key);
    }
    if state.search.is_some() {
        return handle_search_overlay(state, key);
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => Action::Quit,
        (KeyCode::Char('?'), _) => {
            state.show_help = !state.show_help;
            Action::Continue
        }
        (KeyCode::Tab, _) => {
            state.focus = if state.focus == Focus::Tree { Focus::Output } else { Focus::Tree };
            Action::Continue
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => move_selection(state, 1),
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => move_selection(state, -1),
        (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
            state.toggle_fold();
            Action::Continue
        }
        (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
            state.toggle_fold();
            Action::Continue
        }
        (KeyCode::Enter, _) | (KeyCode::Char(' '), _) => { state.toggle_fold(); Action::Continue }
        (KeyCode::Char('g'), _) => { state.scroll_y = 0; Action::Continue }
        (KeyCode::Char('G'), _) => { state.scroll_y = u16::MAX; Action::Continue }
        (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            state.scroll_y = state.scroll_y.saturating_add(10); Action::Continue
        }
        (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            state.scroll_y = state.scroll_y.saturating_sub(10); Action::Continue
        }
        (KeyCode::Char('f'), _) => { state.cycle_filter(); Action::Continue }
        (KeyCode::Char('t'), _) => { state.show_timestamps = !state.show_timestamps; Action::Continue }
        (KeyCode::Char('w'), _) => { state.soft_wrap = !state.soft_wrap; Action::Continue }
        (KeyCode::Char('b'), _) => {
            state.picker = Some(crate::state::PickerState { builds: vec![], cursor: 0 });
            Action::Continue
        }
        (KeyCode::Char('r'), _) => Action::Reload,
        (KeyCode::Char('/'), _) => {
            state.search = Some(crate::state::SearchState {
                pattern: String::new(),
                matches: Vec::new(),
                cursor: 0,
            });
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn move_selection(state: &mut UiState, delta: i32) -> Action {
    let len = state.flat.len() as i32;
    if len == 0 { return Action::Continue; }
    let next = (state.selected as i32 + delta).rem_euclid(len);
    state.selected = next as usize;
    state.scroll_y = 0;
    Action::Continue
}

fn handle_picker(state: &mut UiState, key: KeyEvent) -> Action {
    // Defensive: this is only called when picker is Some(_).
    let target_id = {
        let Some(p) = state.picker.as_mut() else { return Action::Continue };
        match key.code {
            KeyCode::Esc => { state.picker = None; return Action::Continue; }
            KeyCode::Down | KeyCode::Char('j') => {
                if !p.builds.is_empty() { p.cursor = (p.cursor + 1) % p.builds.len(); }
                return Action::Continue;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !p.builds.is_empty() { p.cursor = (p.cursor + p.builds.len() - 1) % p.builds.len(); }
                return Action::Continue;
            }
            KeyCode::Enter => {
                if let Some(target) = p.builds.get(p.cursor).cloned() {
                    target.build_id
                } else {
                    return Action::Continue;
                }
            }
            _ => return Action::Continue,
        }
    };
    state.picker = None;
    Action::SwitchBuild(target_id)
}

fn handle_search_overlay(_state: &mut UiState, _key: KeyEvent) -> Action {
    // Implementation deferred to Task 16.
    Action::Continue
}
