//! `AppState` + `IndexTree`. See design spec §Index, §Camera.

use crate::dag_data::WaveDagData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRow {
    pub node_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeRow {
    pub name: String,
    pub units: Vec<UnitRow>,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveRow {
    pub label: String,
    pub recipes: Vec<RecipeRow>,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexTree {
    pub waves: Vec<WaveRow>,
}

impl IndexTree {
    pub fn from_graph(g: &WaveDagData) -> Self {
        let mut waves = Vec::with_capacity(g.waves.len());
        for (wi, wave) in g.waves.iter().enumerate() {
            let mut recipes: Vec<RecipeRow> = wave
                .recipes
                .iter()
                .map(|name| RecipeRow {
                    name: name.clone(),
                    units: Vec::new(),
                    expanded: false,
                })
                .collect();

            for n in &wave.nodes {
                if n.kind != "unit" {
                    continue;
                }
                let Some(recipe) = n.recipe.as_deref() else {
                    continue;
                };
                let Some(row) = recipes.iter_mut().find(|r| r.name == recipe) else {
                    continue;
                };
                row.units.push(UnitRow {
                    node_id: n.id.clone(),
                    label: n.label.clone(),
                });
            }

            waves.push(WaveRow {
                label: format!("Wave {} ({} recipes)", wi, recipes.len()),
                recipes,
                expanded: wi == 0, // Wave 0 expanded by default.
            });
        }
        Self { waves }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub wave: usize,
    pub recipe: Option<usize>,
    pub unit: Option<usize>,
}

impl Selection {
    pub fn first() -> Self {
        Self { wave: 0, recipe: None, unit: None }
    }

    pub fn node_id<'a>(&self, tree: &'a IndexTree) -> Option<&'a str> {
        let w = tree.waves.get(self.wave)?;
        let r = w.recipes.get(self.recipe?)?;
        let u = r.units.get(self.unit?)?;
        Some(&u.node_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    EdgePicker,
    Help,
    DetailOverlay,
}

pub struct AppState {
    pub tree: IndexTree,
    pub selection: Selection,
    pub mode: Mode,
    pub camera_x: i32,
    pub camera_y: i32,
    pub follow: bool,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(graph: &WaveDagData) -> Self {
        Self {
            tree: IndexTree::from_graph(graph),
            selection: Selection::first(),
            mode: Mode::Normal,
            camera_x: 0,
            camera_y: 0,
            follow: true,
            should_quit: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{NodeData, WaveData, WaveDagData};

    fn unit(id: &str, recipe: &str, label: &str) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "unit".into(),
            label: label.into(),
            recipe: Some(recipe.into()),
            command: Some("cmd".into()),
            output: None,
            cached: Some(true),
            dep_kind: Some("sequential".into()),
            group_index: None,
            modified: None,
        }
    }

    fn graph_2x2() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![
                WaveData {
                    recipes: vec!["a".into(), "b".into()],
                    nodes: vec![
                        unit("unit:a:0", "a", "a0"),
                        unit("unit:a:1", "a", "a1"),
                        unit("unit:b:0", "b", "b0"),
                    ],
                    edges: vec![],
                },
                WaveData {
                    recipes: vec!["c".into()],
                    nodes: vec![unit("unit:c:0", "c", "c0")],
                    edges: vec![],
                },
            ],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn tree_groups_units_by_recipe() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert_eq!(t.waves.len(), 2);
        assert_eq!(t.waves[0].recipes.len(), 2);
        assert_eq!(t.waves[0].recipes[0].name, "a");
        assert_eq!(t.waves[0].recipes[0].units.len(), 2);
        assert_eq!(t.waves[0].recipes[0].units[0].label, "a0");
        assert_eq!(t.waves[0].recipes[1].name, "b");
        assert_eq!(t.waves[0].recipes[1].units.len(), 1);
        assert_eq!(t.waves[1].recipes[0].name, "c");
    }

    #[test]
    fn wave_zero_is_expanded_by_default() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert!(t.waves[0].expanded);
        assert!(!t.waves[1].expanded);
    }

    #[test]
    fn recipes_default_collapsed() {
        let t = IndexTree::from_graph(&graph_2x2());
        for w in &t.waves {
            for r in &w.recipes {
                assert!(!r.expanded);
            }
        }
    }

    #[test]
    fn selection_node_id_returns_unit_id_when_fully_qualified() {
        let t = IndexTree::from_graph(&graph_2x2());
        let sel = Selection { wave: 0, recipe: Some(0), unit: Some(1) };
        assert_eq!(sel.node_id(&t), Some("unit:a:1"));
    }

    #[test]
    fn selection_node_id_is_none_at_wave_or_recipe_level() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert_eq!(Selection { wave: 0, recipe: None, unit: None }.node_id(&t), None);
        assert_eq!(
            Selection { wave: 0, recipe: Some(0), unit: None }.node_id(&t),
            None
        );
    }

    #[test]
    fn app_state_starts_with_first_selection_and_follow_on() {
        let app = AppState::new(&graph_2x2());
        assert_eq!(app.selection, Selection::first());
        assert!(app.follow);
        assert_eq!(app.mode, Mode::Normal);
    }
}
