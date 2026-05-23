//! `cook affected` selection primitive.
//!
//! Computes the subset of a recipe closure whose declared file inputs
//! intersect a set of changed paths, then walks the reverse-DAG to add
//! every transitive downstream consumer.

pub mod compute;
pub mod git;

pub use compute::compute_affected;

#[cfg(test)]
mod tests;
