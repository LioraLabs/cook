// Placeholder until cook-progress::log_reader is added in Task 2.
// For Task 1, expose stubs so `cargo check -p cook-logs` succeeds.

use crate::{error::ViewerError, theme::Theme};

pub fn run(_theme: Theme) -> Result<(), ViewerError> {
    unimplemented!("filled in by later tasks")
}

pub fn run_with_backend(_theme: Theme) -> Result<(), ViewerError> {
    unimplemented!("filled in by later tasks")
}
