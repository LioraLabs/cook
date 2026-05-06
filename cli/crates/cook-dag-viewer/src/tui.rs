//! Terminal init/teardown + event loop — see design spec §TUI.

use crate::frame::ViewFrame;
use crate::ViewerError;

pub fn run<F: ViewFrame>(_frame: F) -> Result<(), ViewerError> {
    // Filled in by Task 9.
    Ok(())
}
