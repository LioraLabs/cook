//! `cook pull` — fetch module subtrees from a configured HTTPS registry into
//! the project-local `cook_modules/` directory.
//!
//! Entry point: [`run_from_argv`].

mod errors;

pub use errors::PullError;

/// Public entry. Returns the process exit code.
pub fn run_from_argv(_argv: &[String]) -> i32 {
    eprintln!("cook pull: not yet implemented");
    1
}
