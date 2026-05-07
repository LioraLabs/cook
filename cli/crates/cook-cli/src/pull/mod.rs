//! `cook pull` — fetch module subtrees from a configured HTTPS registry into
//! the project-local `cook_modules/` directory.

mod args;
mod errors;

pub use errors::PullError;

/// Public entry. Returns the process exit code.
pub fn run_from_argv(argv: &[String]) -> i32 {
    // argv[0] is "pull"; argv[1..] is the pull-args.
    let pull_argv: Vec<String> = argv.iter().skip(1).cloned().collect();
    let args = match args::parse(&pull_argv) {
        Ok(a) => a,
        Err(e) => {
            // BadArgs with empty reason means clap already printed a diagnostic.
            if !matches!(&e, PullError::BadArgs { reason } if reason.is_empty()) {
                eprintln!("cook pull: {e}");
            }
            return e.exit_code();
        }
    };
    eprintln!("cook pull: parsed args ({:?}); orchestration not yet implemented", args);
    1
}
