//! Detail pane renderer. See design spec §Detail Pane.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::dag_data::{NodeData, WaveDagData};
use crate::frame::{NodeStatus, ViewFrame};
use crate::state::AppState;

pub fn render<F: ViewFrame>(area: Rect, buf: &mut Buffer, app: &AppState, frame: &F) {
    let g = frame.graph();
    let Some(node_id) = app.selection.node_id(&app.tree) else {
        write_line(area, buf, area.y, "(no selection)");
        return;
    };
    let Some((node, wave_idx)) = find_node(g, node_id) else {
        write_line(area, buf, area.y, "(node not found)");
        return;
    };

    let status = frame.status_of(node_id);
    let header = format!("{}   {}", node_id, status_label(status));
    let cmd_line = format!(
        "cmd: {}",
        node.command.as_deref().unwrap_or("(no command — file node)")
    );
    let inputs = adjacency(g, wave_idx, node_id, AdjDir::In);
    let consumers = adjacency(g, wave_idx, node_id, AdjDir::Out);
    let inputs_line = format!("inputs ({}):  {}", inputs.len(), inputs.join(" · "));
    let consumers_line =
        format!("consumers ({}):  {}", consumers.len(), consumers.join(" · "));
    let recipe_line = format!(
        "recipe: {}  ·  wave: {}  ·  group: {}",
        node.recipe.as_deref().unwrap_or("-"),
        wave_idx,
        group_label(node)
    );

    let lines = [
        header.as_str(),
        cmd_line.as_str(),
        inputs_line.as_str(),
        consumers_line.as_str(),
        recipe_line.as_str(),
    ];
    for (i, line) in lines.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        write_line(area, buf, y, line);
    }
}

enum AdjDir {
    In,
    Out,
}

fn adjacency(g: &WaveDagData, wave_idx: usize, node_id: &str, dir: AdjDir) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(wave) = g.waves.get(wave_idx) {
        for e in &wave.edges {
            match dir {
                AdjDir::In if e.to == node_id => out.push(e.from.clone()),
                AdjDir::Out if e.from == node_id => out.push(e.to.clone()),
                _ => {}
            }
        }
    }
    for e in &g.inter_wave_edges {
        match dir {
            AdjDir::In if e.to == node_id => out.push(e.from.clone()),
            AdjDir::Out if e.from == node_id => out.push(e.to.clone()),
            _ => {}
        }
    }
    out
}

fn find_node<'a>(g: &'a WaveDagData, id: &str) -> Option<(&'a NodeData, usize)> {
    for (wi, wave) in g.waves.iter().enumerate() {
        for n in &wave.nodes {
            if n.id == id {
                return Some((n, wi));
            }
        }
    }
    None
}

fn status_label(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Cached => "✓ cached",
        NodeStatus::Stale => "✗ stale",
        NodeStatus::Modified => "⚠ modified",
        NodeStatus::Done => "· done",
        NodeStatus::Pending => "· pending",
        NodeStatus::Running => "▶ running",
        NodeStatus::Failed => "✗ failed",
    }
}

fn group_label(node: &NodeData) -> String {
    match (node.dep_kind.as_deref(), node.group_index) {
        (Some("step_group"), Some(g)) => format!("step-group #{g}"),
        (Some("test_sibling"), Some(g)) => format!("test-sibling #{g}"),
        _ => "sequential".into(),
    }
}

fn write_line(area: Rect, buf: &mut Buffer, y: u16, text: &str) {
    let max = area.x + area.width;
    let mut col = area.x;
    for ch in text.chars() {
        if col >= max {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch).set_style(Style::default());
        }
        col += 1;
    }
    while col < max {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(' ').set_style(Style::default());
        }
        col += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
    use crate::frame::SnapshotFrame;
    use crate::state::{AppState, Selection};

    fn graph() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    NodeData {
                        id: "file:foo.cpp".into(),
                        kind: "file".into(),
                        label: "foo.cpp".into(),
                        recipe: None,
                        command: None,
                        output: None,
                        cached: None,
                        dep_kind: None,
                        group_index: None,
                        modified: Some(false),
                        discovered: None,
                    },
                    NodeData {
                        id: "unit:a:0".into(),
                        kind: "unit".into(),
                        label: "foo.o".into(),
                        recipe: Some("a".into()),
                        command: Some("clang -c foo.cpp".into()),
                        output: Some("foo.o".into()),
                        cached: Some(true),
                        dep_kind: Some("sequential".into()),
                        group_index: None,
                        modified: None,
                        discovered: None,
                    },
                ],
                edges: vec![EdgeData {
                    from: "file:foo.cpp".into(),
                    to: "unit:a:0".into(),
                }],
            }],
            inter_wave_edges: vec![],
        }
    }

    fn first_line(buf: &Buffer, area: Rect) -> String {
        (area.x..area.x + area.width)
            .map(|x| buf.cell((x, area.y)).unwrap().symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn renders_header_and_inputs() {
        let g = graph();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 80, 6);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        assert!(first_line(&buf, area).contains("unit:a:0"));
        assert!(first_line(&buf, area).contains("✓ cached"));
        // Row 2 should mention the file input.
        let row2: String = (0..80)
            .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
            .collect();
        assert!(row2.contains("file:foo.cpp"));
    }

    #[test]
    fn renders_no_selection_message_when_at_wave_level() {
        let g = graph();
        let app = AppState::new(&g); // first() => wave-level selection
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);
        assert!(first_line(&buf, area).contains("(no selection)"));
    }
}
