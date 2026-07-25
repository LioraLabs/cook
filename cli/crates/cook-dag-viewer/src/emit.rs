//! Aggregation and rendering for `cook dag`.
//!
//! The unit-level graph is the truth, but it is rarely the thing to look at:
//! DuckDB registers 1711 units, which no layout — terminal, mermaid, or
//! otherwise — renders usefully. So the graph is *collapsed* first and
//! rendered second, and the default collapse is coarse enough to fit on a
//! screen.
//!
//! Aggregation preserves the one thing that makes the graph diagnostic: when a
//! bundle of edges collapses to a single arrow, the arrow keeps the *most
//! constraining* kind in the bundle (`EdgeKind` is ordered for exactly this).
//! A whole-recipe barrier hiding inside a bundle of data edges is the case
//! worth seeing, so it is the case that survives.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::dag_data::{EdgeKind, WaveDagData};

/// How much of the graph to collapse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// One node per recipe. The default: it answers "what waits on what, and
    /// why" without drowning the answer.
    Recipe,
    /// One node per step group, plus one per non-grouped unit. Shows where a
    /// recipe's parallelism actually is.
    Group,
    /// Every unit. Honest, and unreadable past a few hundred nodes — see
    /// [`UNIT_LEVEL_SOFT_CAP`].
    Unit,
}

/// Output syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Mermaid,
    Dot,
    Json,
}

/// Above this many nodes, `--level unit` refuses rather than emitting
/// something no one can read. Chosen to be comfortably past a hand-written
/// Cookfile and far below a real C++ tree.
pub const UNIT_LEVEL_SOFT_CAP: usize = 200;

/// A collapsed node.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub label: String,
    /// How many unit-level nodes collapsed into this one.
    pub units: usize,
    /// `Some(true)` if every collapsed unit was cached, `Some(false)` if none
    /// were, `None` if mixed or unknown.
    pub cached: Option<bool>,
}

/// A collapsed edge. `count` is how many unit-level edges it stands for.
#[derive(Debug, Clone)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub count: usize,
}

/// The collapsed graph handed to a renderer.
#[derive(Debug, Clone)]
pub struct Graph {
    pub target: String,
    pub level: Level,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Unit-level node count before collapsing, for the summary line.
    pub total_units: usize,
}

/// Refusal from [`aggregate`] — currently only the unit-level cap.
#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    #[error(
        "--level unit would emit {nodes} nodes, which is not readable in any format.\n\
         Use --level recipe (the default) or --level group, or raise the cap with \
         --max-nodes <N> if you really want the full graph."
    )]
    TooManyNodes { nodes: usize },
}

/// Map a unit-level node id to its collapsed id at `level`, or `None` when the
/// node is dropped by that level.
fn collapse_id(
    level: Level,
    node: &crate::dag_data::NodeData,
) -> Option<String> {
    // File nodes are dropped above unit level: at recipe or group granularity
    // they outnumber the units several times over and say nothing about
    // ordering, which is the question these levels exist to answer.
    if node.kind != "unit" {
        return match level {
            Level::Unit => Some(node.id.clone()),
            _ => None,
        };
    }
    let recipe = node.recipe.clone().unwrap_or_default();
    match level {
        Level::Recipe => Some(format!("recipe:{recipe}")),
        Level::Group => match node.group_index {
            Some(gi) => Some(format!("group:{recipe}:{gi}")),
            None => Some(node.id.clone()),
        },
        Level::Unit => Some(node.id.clone()),
    }
}

/// Collapse the unit-level graph to `level`.
pub fn aggregate(dag: &WaveDagData, level: Level, max_nodes: usize) -> Result<Graph, EmitError> {
    // unit-node id -> collapsed id
    let mut mapping: BTreeMap<&str, String> = BTreeMap::new();
    // collapsed id -> (label, unit count, cached tally)
    let mut acc: BTreeMap<String, (String, usize, Vec<Option<bool>>)> = BTreeMap::new();
    let mut total_units = 0usize;

    for wave in &dag.waves {
        for node in &wave.nodes {
            if node.kind == "unit" {
                total_units += 1;
            }
            let Some(cid) = collapse_id(level, node) else {
                continue;
            };
            mapping.insert(node.id.as_str(), cid.clone());
            let label = match level {
                Level::Recipe => node.recipe.clone().unwrap_or_else(|| node.label.clone()),
                Level::Group => match node.group_index {
                    Some(gi) => format!("{}#{}", node.recipe.clone().unwrap_or_default(), gi),
                    None => node.label.clone(),
                },
                Level::Unit => node.label.clone(),
            };
            let entry = acc.entry(cid).or_insert_with(|| (label, 0, Vec::new()));
            entry.1 += 1;
            entry.2.push(node.cached);
        }
    }

    if level == Level::Unit && acc.len() > max_nodes {
        return Err(EmitError::TooManyNodes { nodes: acc.len() });
    }

    let nodes: Vec<Node> = acc
        .into_iter()
        .map(|(id, (label, units, cached))| {
            let all = cached.iter().all(|c| *c == Some(true));
            let none = cached.iter().all(|c| *c == Some(false));
            Node {
                id,
                label,
                units,
                cached: if all {
                    Some(true)
                } else if none {
                    Some(false)
                } else {
                    None
                },
            }
        })
        .collect();

    // Merge edges between collapsed endpoints. The surviving kind is the max
    // (most constraining) of the bundle — a barrier must not disappear behind
    // the data edges it travels with.
    let mut merged: BTreeMap<(String, String), (EdgeKind, usize)> = BTreeMap::new();
    let all_edges = dag
        .waves
        .iter()
        .flat_map(|w| w.edges.iter())
        .chain(dag.inter_wave_edges.iter());
    for e in all_edges {
        let (Some(from), Some(to)) = (mapping.get(e.from.as_str()), mapping.get(e.to.as_str()))
        else {
            continue;
        };
        // A collapsed self-edge is an artifact of collapsing, not a real
        // dependency — the units inside one recipe or group ordering among
        // themselves is exactly what this level chose to hide.
        if from == to {
            continue;
        }
        let entry = merged
            .entry((from.clone(), to.clone()))
            .or_insert((e.kind, 0));
        if e.kind > entry.0 {
            entry.0 = e.kind;
        }
        entry.1 += 1;
    }

    let edges: Vec<Edge> = merged
        .into_iter()
        .map(|((from, to), (kind, count))| Edge {
            from,
            to,
            kind,
            count,
        })
        .collect();

    Ok(Graph {
        target: dag.target.clone(),
        level,
        nodes,
        edges,
        total_units,
    })
}

/// Render `graph` in `format`.
pub fn render(graph: &Graph, format: Format) -> String {
    match format {
        Format::Text => render_text(graph),
        Format::Mermaid => render_mermaid(graph),
        Format::Dot => render_dot(graph),
        Format::Json => render_json(graph),
    }
}

fn cache_marker(n: &Node) -> &'static str {
    match n.cached {
        Some(true) => " [cached]",
        Some(false) => " [stale]",
        None => "",
    }
}

fn unit_suffix(n: &Node) -> String {
    if n.units > 1 {
        format!(" ×{}", n.units)
    } else {
        String::new()
    }
}

/// Indented, greppable. The default, because the usual use is a quick look in
/// a terminal followed by a grep.
fn render_text(graph: &Graph) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} — {} node(s) at {} level, {} unit(s) total",
        graph.target,
        graph.nodes.len(),
        match graph.level {
            Level::Recipe => "recipe",
            Level::Group => "group",
            Level::Unit => "unit",
        },
        graph.total_units
    );

    // Incoming edges per node, so each node reads as "this waits on that".
    let mut incoming: BTreeMap<&str, Vec<&Edge>> = BTreeMap::new();
    for e in &graph.edges {
        incoming.entry(e.to.as_str()).or_default().push(e);
    }
    let label_of: BTreeMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.label.as_str()))
        .collect();

    for n in &graph.nodes {
        let _ = writeln!(
            out,
            "\n{}{}{}",
            n.label,
            unit_suffix(n),
            cache_marker(n)
        );
        let deps = incoming.get(n.id.as_str());
        match deps {
            None => {
                // At recipe/group level file inputs are collapsed away, so
                // "nothing" here means nothing *orders* this node — not that
                // it has no inputs. Say which.
                let _ = writeln!(
                    out,
                    "  {}",
                    match graph.level {
                        Level::Unit => "(waits on nothing)",
                        _ => "(waits on nothing — free to start immediately)",
                    }
                );
            }
            Some(list) => {
                for e in list {
                    let from = label_of.get(e.from.as_str()).copied().unwrap_or(&e.from);
                    let count = if e.count > 1 {
                        format!(" ×{}", e.count)
                    } else {
                        String::new()
                    };
                    let _ = writeln!(
                        out,
                        "  waits on {:<24} {}{}",
                        from,
                        e.kind.label(),
                        count
                    );
                }
            }
        }
    }
    out
}

/// Sanitise an id for mermaid/dot, which reject most punctuation in node ids.
fn safe_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Arrow syntax per kind: ordering edges are visually heavier than data ones,
/// and a whole-recipe barrier is the heaviest thing on the diagram.
fn mermaid_arrow(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Barrier => "==>",
        EdgeKind::DepOrder | EdgeKind::Serial => "-->",
        EdgeKind::Group => "-->",
        EdgeKind::Data | EdgeKind::Discovered | EdgeKind::Probe => "-.->",
    }
}

fn render_mermaid(graph: &Graph) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "graph LR");
    for n in &graph.nodes {
        let _ = writeln!(
            out,
            "  {}[\"{}{}\"]",
            safe_id(&n.id),
            n.label.replace('"', "'"),
            unit_suffix(n)
        );
    }
    for e in &graph.edges {
        let count = if e.count > 1 {
            format!(" ×{}", e.count)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "  {} {}|{}{}| {}",
            safe_id(&e.from),
            mermaid_arrow(e.kind),
            e.kind.label(),
            count,
            safe_id(&e.to)
        );
    }
    // Make barriers legible at a glance without requiring the reader to
    // decode arrow weights.
    for (i, e) in graph.edges.iter().enumerate() {
        if e.kind == EdgeKind::Barrier {
            let _ = writeln!(out, "  linkStyle {i} stroke-width:3px");
        }
    }
    out
}

fn render_dot(graph: &Graph) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "digraph cook {{");
    let _ = writeln!(out, "  rankdir=LR;");
    let _ = writeln!(out, "  node [shape=box, fontname=\"sans\"];");
    for n in &graph.nodes {
        let _ = writeln!(
            out,
            "  {} [label=\"{}{}\"];",
            safe_id(&n.id),
            n.label.replace('"', "'"),
            unit_suffix(n)
        );
    }
    for e in &graph.edges {
        let style = match e.kind {
            EdgeKind::Barrier => ", penwidth=3",
            EdgeKind::Data | EdgeKind::Discovered | EdgeKind::Probe => ", style=dashed",
            _ => "",
        };
        let count = if e.count > 1 {
            format!(" ×{}", e.count)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "  {} -> {} [label=\"{}{}\"{}];",
            safe_id(&e.from),
            safe_id(&e.to),
            e.kind.label(),
            count,
            style
        );
    }
    let _ = writeln!(out, "}}");
    out
}

fn render_json(graph: &Graph) -> String {
    // Hand-rolled rather than derived: this is a published surface and its
    // shape should not drift with an internal struct rename.
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "  \"target\": {:?},", graph.target);
    let _ = writeln!(
        out,
        "  \"level\": {:?},",
        match graph.level {
            Level::Recipe => "recipe",
            Level::Group => "group",
            Level::Unit => "unit",
        }
    );
    let _ = writeln!(out, "  \"total_units\": {},", graph.total_units);
    let _ = writeln!(out, "  \"nodes\": [");
    for (i, n) in graph.nodes.iter().enumerate() {
        let comma = if i + 1 < graph.nodes.len() { "," } else { "" };
        let cached = match n.cached {
            Some(b) => b.to_string(),
            None => "null".to_string(),
        };
        let _ = writeln!(
            out,
            "    {{\"id\": {:?}, \"label\": {:?}, \"units\": {}, \"cached\": {}}}{}",
            n.id, n.label, n.units, cached, comma
        );
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"edges\": [");
    for (i, e) in graph.edges.iter().enumerate() {
        let comma = if i + 1 < graph.edges.len() { "," } else { "" };
        let _ = writeln!(
            out,
            "    {{\"from\": {:?}, \"to\": {:?}, \"kind\": {:?}, \"count\": {}}}{}",
            e.from,
            e.to,
            e.kind.label(),
            e.count,
            comma
        );
    }
    let _ = writeln!(out, "  ]");
    let _ = writeln!(out, "}}");
    out
}

#[cfg(test)]
#[path = "tests/emit_tests.rs"]
mod tests;
