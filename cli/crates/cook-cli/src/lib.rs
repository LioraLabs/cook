//! Library surface for `cook-cli` integration tests. The binary crate is the
//! product; this lib exists so integration tests can drive `pull::run_from_argv`
//! without spawning a subprocess.

pub mod modules;
pub mod pull;
