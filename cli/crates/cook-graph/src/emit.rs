//! Aggregation and rendering for `cook why`.
//!
//! The unit-level graph is the truth, but it is rarely the thing to look at:
//! DuckDB registers 1711 units, which no layout — terminal, mermaid, or
//! otherwise — renders usefully. So the graph is *collapsed* first and
//! rendered second, and the default collapse is coarse enough to fit on a
//! screen. This is what lets per-unit cache fidelity be readable at all: the
//! flat per-unit report `cook why` used to emit had no ceiling and no
//! aggregation, and on a tree of this size it was thousands of blocks.
//!
//! Aggregation preserves the two things that make the graph diagnostic:
//!
//! * When a bundle of edges collapses to a single arrow, the arrow keeps the
//!   *most constraining* kind in the bundle (`EdgeKind` is ordered for exactly
//!   this). A whole-recipe barrier hiding inside a bundle of data edges is the
//!   case worth seeing, so it is the case that survives.
//! * A collapsed node keeps hit and rebuild *counts*, never a boolean. A
//!   recipe with 339 hits and one rebuild is not the same finding as a recipe
//!   with 340 rebuilds, and §17.1.6.2 requires the two to be distinguishable.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use cook_engine::timings::render_ms;

use crate::annotate::Annotations;
use crate::dag_data::{DagData, EdgeKind};

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
    /// Collapsed units that will be served from cache.
    pub hits: usize,
    /// Collapsed units that will rebuild.
    pub rebuilds: usize,
    /// Collapsed units with no cache verdict — a non-cacheable step, or a unit
    /// the annotation set never covered. Counted rather than folded into
    /// either tally: §17.1.6.2 requires a mixed node to be distinguishable
    /// from a uniform one, and quietly calling an unclassified unit a rebuild
    /// would manufacture exactly that confusion.
    pub unclassified: usize,
    /// Summed last-observed wall time over the collapsed units that have an
    /// observation.
    pub observed_ms: u64,
    /// Collapsed units with no observation at all. A caller rendering
    /// `observed_ms` MUST surface this too: a sum over three of forty units is
    /// not this node's build time (§17.1.6.4).
    pub unobserved: usize,
    /// Staleness bound on `observed_ms`: the age, in retained builds, of the
    /// OLDEST observation summed into it. `0` means every contributing unit was
    /// timed in the most recent build.
    ///
    /// The maximum rather than the minimum, because this is the figure that
    /// tells a reader how much to distrust the total, and the weakest
    /// contributor sets that.
    pub observed_max_age: usize,
    /// Units in nodes strictly downstream of this one — what this node's
    /// rebuild invalidates in the rest of the graph (§17.1.6.3). Filled by
    /// [`cascade`]; zero until then.
    ///
    /// Counts *all* downstream units, not only the ones already classified as
    /// rebuilding. A downstream unit is usually still a cache hit at the
    /// moment the query runs — its inputs have not changed *yet*, because the
    /// upstream rebuild has not happened. It is precisely the units that look
    /// like hits now and will not survive the run that a reader needs told
    /// about, so counting only the current rebuilds would report zero in the
    /// exact case the number exists for.
    ///
    /// It is an upper bound rather than a certainty: a rebuild that reproduces
    /// byte-identical output leaves its consumers' keys unchanged. The
    /// rendering says "invalidates" rather than "will rebuild" for that reason.
    pub forces: usize,
}

impl Node {
    /// True when this node has work that will actually run.
    pub fn rebuilding(&self) -> bool {
        self.rebuilds > 0
    }
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

/// Collapse the unit-level graph to `level`, rolling `annotations` up as it
/// goes.
///
/// Pass [`Annotations::new`] for a structure-only view; every unit then lands
/// in `unclassified` and no cache or timing claim is made.
pub fn aggregate(
    dag: &DagData,
    level: Level,
    max_nodes: usize,
    annotations: &Annotations,
) -> Result<Graph, EmitError> {
    // unit-node id -> collapsed id
    let mut mapping: BTreeMap<&str, String> = BTreeMap::new();
    let mut acc: BTreeMap<String, Node> = BTreeMap::new();
    let mut total_units = 0usize;

    for node in &dag.nodes {
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
        let entry = acc.entry(cid.clone()).or_insert_with(|| Node {
            id: cid,
            label,
            units: 0,
            hits: 0,
            rebuilds: 0,
            unclassified: 0,
            observed_ms: 0,
            unobserved: 0,
            observed_max_age: 0,
            forces: 0,
        });
        // A file node is structure, not work: it has no cache verdict and no
        // runtime, so it counts toward nothing — not even `units`. Counting it
        // would give it an "observed" span of zero units taking zero time,
        // which renders as "0ms observed" and is precisely the absence-as-zero
        // §17.1.6.4 forbids. File nodes only reach here at unit level, where
        // they survive collapsing.
        if node.kind != "unit" {
            continue;
        }
        entry.units += 1;
        match annotations.for_node(node) {
            Some(f) => {
                if f.served {
                    entry.hits += 1;
                } else {
                    entry.rebuilds += 1;
                }
                match f.observed_ms {
                    Some(ms) => {
                        entry.observed_ms += ms;
                        entry.observed_max_age =
                            entry.observed_max_age.max(f.observed_builds_ago);
                    }
                    None => entry.unobserved += 1,
                }
            }
            None => {
                entry.unclassified += 1;
                entry.unobserved += 1;
            }
        }
    }

    if level == Level::Unit && acc.len() > max_nodes {
        return Err(EmitError::TooManyNodes { nodes: acc.len() });
    }

    let nodes: Vec<Node> = acc.into_values().collect();

    // Merge edges between collapsed endpoints. The surviving kind is the max
    // (most constraining) of the bundle — a barrier must not disappear behind
    // the data edges it travels with.
    let mut merged: BTreeMap<(String, String), (EdgeKind, usize)> = BTreeMap::new();
    for e in &dag.edges {
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

    let mut graph = Graph {
        target: dag.target.clone(),
        level,
        nodes,
        edges,
        total_units,
    };
    cascade(&mut graph);
    Ok(graph)
}

/// Fill each node's `forces`: how many units sit strictly downstream of it
/// (§17.1.6.3).
///
/// Run over the *collapsed* graph rather than the unit graph, which is what
/// makes a per-node reachability walk affordable: the collapsed graph is a few
/// nodes at recipe level and capped at `max_nodes` at unit level, where the
/// unit graph it came from may hold thousands.
///
/// The number answers "what does this rebuild cost the rest of the build",
/// which is the question a single changed input raises and which no per-vertex
/// miss report can answer.
fn cascade(graph: &mut Graph) {
    let index: BTreeMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    for e in &graph.edges {
        if let (Some(&f), Some(&t)) = (index.get(e.from.as_str()), index.get(e.to.as_str())) {
            succ[f].push(t);
        }
    }

    let units: Vec<usize> = graph.nodes.iter().map(|n| n.units).collect();
    let mut forces = vec![0usize; graph.nodes.len()];
    for start in 0..graph.nodes.len() {
        // Iterative DFS: the collapsed graph is small, but a recipe graph is
        // still author-shaped and recursion depth should not depend on it.
        let mut seen = vec![false; graph.nodes.len()];
        let mut stack = vec![start];
        seen[start] = true;
        let mut total = 0;
        while let Some(n) = stack.pop() {
            for &m in &succ[n] {
                if !seen[m] {
                    seen[m] = true;
                    total += units[m];
                    stack.push(m);
                }
            }
        }
        forces[start] = total;
    }
    for (n, f) in graph.nodes.iter_mut().zip(forces) {
        n.forces = f;
    }
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

/// The cache verdict for a collapsed node, as a tally rather than a boolean.
///
/// §17.1.6.2 requires a mixed node to be distinguishable from a uniform one,
/// which is precisely what the `[cached]`/`[stale]` marker this replaced could
/// not do: a recipe with 339 hits and one rebuild rendered identically to a
/// recipe with 340 rebuilds.
fn cache_summary(n: &Node) -> String {
    if n.units == 0 || (n.hits == 0 && n.rebuilds == 0) {
        return String::new();
    }
    let mut s = if n.rebuilds == 0 {
        format!("{} hit", n.hits)
    } else if n.hits == 0 {
        format!("{} rebuild", n.rebuilds)
    } else {
        format!("{} hit / {} rebuild", n.hits, n.rebuilds)
    };
    if n.unclassified > 0 {
        let _ = write!(s, " / {} unclassified", n.unclassified);
    }
    s
}

/// The timing column, or empty when nothing was ever observed.
///
/// Always says "observed", and always admits how much of the node it does not
/// cover. §17.1.6.4 forbids presenting this as a prediction, and a bare
/// duration next to a rebuild count would read as exactly that.
fn timing_summary(n: &Node) -> String {
    let observed = n.units.saturating_sub(n.unobserved);
    if observed == 0 {
        return String::new();
    }
    let mut s = format!("{} observed", render_ms(n.observed_ms));
    if n.unobserved > 0 {
        let _ = write!(s, " ({} of {} units)", observed, n.units);
    }
    // Only worth saying when the number is not from the last run; on a freshly
    // built tree every observation is current and the note would be noise.
    match n.observed_max_age {
        0 => {}
        1 => s.push_str(", 1 build ago"),
        age => {
            let _ = write!(s, ", up to {age} builds ago");
        }
    }
    s
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
    let total_rebuilds: usize = graph.nodes.iter().map(|n| n.rebuilds).sum();
    let total_hits: usize = graph.nodes.iter().map(|n| n.hits).sum();
    let _ = writeln!(
        out,
        "why {} — {} node(s) at {} level, {} unit(s): {} hit, {} rebuild",
        graph.target,
        graph.nodes.len(),
        match graph.level {
            Level::Recipe => "recipe",
            Level::Group => "group",
            Level::Unit => "unit",
        },
        graph.total_units,
        total_hits,
        total_rebuilds,
    );

    // Incoming edges per node, so each node reads as "this waits on that".
    let mut incoming: BTreeMap<&str, Vec<&Edge>> = BTreeMap::new();
    for e in &graph.edges {
        incoming.entry(e.to.as_str()).or_default().push(e);
    }
    let node_of: BTreeMap<&str, &Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    for n in &graph.nodes {
        let mut header = format!("{}{}", n.label, unit_suffix(n));
        let cache = cache_summary(n);
        if !cache.is_empty() {
            let _ = write!(header, "  [{cache}]");
        }
        let timing = timing_summary(n);
        if !timing.is_empty() {
            let _ = write!(header, "  {timing}");
        }
        let _ = writeln!(out, "\n{header}");

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
                    let upstream = node_of.get(e.from.as_str()).copied();
                    let from = upstream.map(|u| u.label.as_str()).unwrap_or(&e.from);
                    let count = if e.count > 1 {
                        format!(" ×{}", e.count)
                    } else {
                        String::new()
                    };
                    // §17.1.6.3: mark the upstream that will itself rebuild.
                    // This is the attribution — a node that rebuilds *because*
                    // of something upstream should name it rather than present
                    // its own miss as an independent finding.
                    let forced = if upstream.is_some_and(|u| u.rebuilding()) {
                        "  ← rebuilding"
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        out,
                        "  waits on {:<24} {}{}{}",
                        from,
                        e.kind.label(),
                        count,
                        forced
                    );
                }
            }
        }
        if n.rebuilding() && n.forces > 0 {
            // "invalidates", not "will rebuild": a rebuild that reproduces
            // byte-identical output leaves its consumers cached, so this is an
            // upper bound and the wording must not promise otherwise.
            let _ = writeln!(
                out,
                "  rebuilding this invalidates {} downstream unit{}",
                n.forces,
                if n.forces == 1 { "" } else { "s" }
            );
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
        let cache = cache_summary(n);
        let suffix = if cache.is_empty() {
            String::new()
        } else {
            format!("<br/>{cache}")
        };
        let _ = writeln!(
            out,
            "  {}[\"{}{}{}\"]",
            safe_id(&n.id),
            n.label.replace('"', "'"),
            unit_suffix(n),
            suffix
        );
    }
    // Nodes that will actually run are the ones a reader is looking for, so
    // they are the ones the diagram distinguishes.
    for n in &graph.nodes {
        if n.rebuilding() {
            let _ = writeln!(
                out,
                "  style {} stroke-width:3px",
                safe_id(&n.id)
            );
        }
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

/// The graph half of the machine-readable payload, as a value so the caller
/// can graft the determinant half onto it (§17.1.6.5: one document, not two).
///
/// Keys serialise sorted — `serde_json::Map` is `BTreeMap`-backed in this
/// build (no `preserve_order` feature) — which is what makes the output
/// byte-stable for a given graph.
pub fn json_value(graph: &Graph) -> serde_json::Value {
    let nodes: Vec<serde_json::Value> = graph
        .nodes
        .iter()
        .map(|n| {
            // `observed_ms` is deliberately paired with `unobserved` in the
            // same object: a consumer reading the duration without reading how
            // many units it failed to cover would be reading a number that does
            // not mean what it looks like (§17.1.6.4).
            serde_json::json!({
                "id": n.id,
                "label": n.label,
                "units": n.units,
                "hits": n.hits,
                "rebuilds": n.rebuilds,
                "unclassified": n.unclassified,
                "forces": n.forces,
                "observed_ms": n.observed_ms,
                "unobserved": n.unobserved,
                "observed_max_age": n.observed_max_age,
            })
        })
        .collect();
    let edges: Vec<serde_json::Value> = graph
        .edges
        .iter()
        .map(|e| {
            serde_json::json!({
                "from": e.from,
                "to": e.to,
                "kind": e.kind.label(),
                "count": e.count,
            })
        })
        .collect();
    serde_json::json!({
        "schema_version": crate::DAG_SCHEMA_VERSION,
        "target": graph.target,
        "level": match graph.level {
            Level::Recipe => "recipe",
            Level::Group => "group",
            Level::Unit => "unit",
        },
        "total_units": graph.total_units,
        "nodes": nodes,
        "edges": edges,
    })
}

fn render_json(graph: &Graph) -> String {
    let mut s = serde_json::to_string_pretty(&json_value(graph)).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
#[path = "tests/emit_tests.rs"]
mod tests;
