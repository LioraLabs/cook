//! Fuzzy-search popup state. See design spec §Search.

use nucleo_matcher::{Config, Matcher, Utf32String};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::dag_data::WaveDagData;

#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub matches: Vec<String>, // node ids
    pub cursor: usize,
}

impl SearchState {
    pub fn update(&mut self, graph: &WaveDagData) {
        self.matches.clear();
        self.cursor = 0;
        if self.query.is_empty() {
            return;
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let needle = Utf32String::from(self.query.as_str());
        let mut scored: Vec<(i32, String)> = Vec::new();
        for wave in &graph.waves {
            for n in &wave.nodes {
                let hay = format!(
                    "{} {} {}",
                    n.label,
                    n.command.as_deref().unwrap_or(""),
                    n.recipe.as_deref().unwrap_or("")
                );
                let utf32 = Utf32String::from(hay.as_str());
                if let Some(score) =
                    matcher.fuzzy_match(utf32.slice(..), needle.slice(..))
                {
                    scored.push((score as i32, n.id.clone()));
                }
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        self.matches = scored.into_iter().map(|(_, id)| id).collect();
    }
}

pub fn render(area: Rect, buf: &mut Buffer, state: &SearchState) {
    let popup = centered_rect(70, 12.min(area.height), area);
    Clear.render(popup, buf);
    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain);
    let inner = block.inner(popup);
    block.render(popup, buf);
    write_line(inner, buf, inner.y, &format!("/{}", state.query));
    for (i, id) in state
        .matches
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
    {
        let y = inner.y + 1 + i as u16;
        let style = if i == state.cursor {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let prefix = if i == state.cursor { '▶' } else { ' ' };
        write_line_styled(inner, buf, y, &format!("{} {}", prefix, id), style);
    }
}

fn write_line(area: Rect, buf: &mut Buffer, y: u16, text: &str) {
    write_line_styled(area, buf, y, text, Style::default());
}

fn write_line_styled(area: Rect, buf: &mut Buffer, y: u16, text: &str, style: Style) {
    let max = area.x + area.width;
    let mut col = area.x;
    for ch in text.chars() {
        if col >= max {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch).set_style(style);
        }
        col += 1;
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{NodeData, WaveData, WaveDagData};

    fn dag() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["cpp.compile".into()],
                nodes: vec![
                    NodeData {
                        id: "unit:cpp.compile:0".into(),
                        kind: "unit".into(),
                        label: "foo.o".into(),
                        recipe: Some("cpp.compile".into()),
                        command: Some("clang -c foo.cpp".into()),
                        output: None,
                        cached: Some(true),
                        dep_kind: Some("sequential".into()),
                        group_index: None,
                        modified: None,
                    },
                    NodeData {
                        id: "unit:cpp.compile:1".into(),
                        kind: "unit".into(),
                        label: "bar.o".into(),
                        recipe: Some("cpp.compile".into()),
                        command: Some("clang -c bar.cpp".into()),
                        output: None,
                        cached: Some(false),
                        dep_kind: Some("sequential".into()),
                        group_index: None,
                        modified: None,
                    },
                ],
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn fuzzy_match_finds_substring() {
        let g = dag();
        let mut s = SearchState::default();
        s.query = "bar".into();
        s.update(&g);
        assert!(s.matches.contains(&"unit:cpp.compile:1".to_string()));
    }

    #[test]
    fn empty_query_returns_no_matches() {
        let g = dag();
        let mut s = SearchState::default();
        s.update(&g);
        assert!(s.matches.is_empty());
    }
}
