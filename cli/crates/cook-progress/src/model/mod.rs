//! Pure state model — see docs/superpowers/specs/2026-04-20-cook-indicatif-rewrite-design.md
pub mod build;
pub mod node;
pub mod recipe;

pub use build::{BuildState, Counters};
pub use node::{NodeState, NodeStatus};
pub use recipe::{RecipeState, Status};
