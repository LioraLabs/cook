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
    let (declared_inputs, discovered_inputs) = split_inputs_by_discovered(g, &inputs);
    let consumers = adjacency(g, wave_idx, node_id, AdjDir::Out);
    let inputs_line = format!(
        "inputs ({}):  {}",
        declared_inputs.len(),
        declared_inputs.join(" · ")
    );
    let discovered_line = format!(
        "discovered ({}):  {}",
        discovered_inputs.len(),
        discovered_inputs.join(" · ")
    );
    let consumers_line =
        format!("consumers ({}):  {}", consumers.len(), consumers.join(" · "));
    let recipe_line = format!(
        "recipe: {}  ·  wave: {}  ·  group: {}",
        node.recipe.as_deref().unwrap_or("-"),
        wave_idx,
        group_label(node)
    );

    let mut lines: Vec<&str> = Vec::with_capacity(6);
    lines.push(header.as_str());
    lines.push(cmd_line.as_str());
    lines.push(inputs_line.as_str());
    if !discovered_inputs.is_empty() {
        lines.push(discovered_line.as_str());
    }
    lines.push(consumers_line.as_str());
    lines.push(recipe_line.as_str());

    for (i, line) in lines.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        write_line(area, buf, y, line);
    }
}

/// Partition a unit's incoming-edge node IDs into (declared, discovered)
/// based on the `discovered` flag of each `from` node in the graph.
fn split_inputs_by_discovered(
    g: &WaveDagData,
    incoming: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut declared = Vec::new();
    let mut discovered = Vec::new();
    for id in incoming {
        let is_discovered = g.waves.iter().any(|w| {
            w.nodes
                .iter()
                .any(|n| n.id == *id && n.discovered == Some(true))
        });
        if is_discovered {
            discovered.push(id.clone());
        } else {
            declared.push(id.clone());
        }
    }
    (declared, discovered)
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
#[path = "tests/detail_tests.rs"]
mod tests;
