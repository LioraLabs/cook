//! Renderer trait and implementations.
pub mod inline;
pub mod plain;
pub mod json;
pub mod event_writer;
pub mod snapshot;
pub mod status_line;

#[cfg(test)]
pub mod test_term;

use std::io;

use crate::event::ProgressEvent;
use crate::model::build::BuildState;

/// ANSI sequence to move cursor to start of line and erase the line.
/// Used by both the inline renderer (clear before each event line) and
/// the status-line tick thread (clear-and-redraw on each tick).
pub(crate) const CLEAR_LINE: &str = "\r\x1b[2K";

pub trait Renderer: Send {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()>;
    fn finish(&mut self, state: &BuildState) -> io::Result<()>;
}
