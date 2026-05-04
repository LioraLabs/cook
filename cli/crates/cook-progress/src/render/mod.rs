//! Renderer trait and implementations.
pub mod inline;
pub mod plain;
pub mod json;
pub mod event_writer;
pub mod snapshot;

#[cfg(test)]
pub mod test_term;

use std::io;

use crate::event::ProgressEvent;
use crate::model::build::BuildState;

pub trait Renderer: Send {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()>;
    fn finish(&mut self, state: &BuildState) -> io::Result<()>;
}
