//! `cook modules` — install the Lua modules a project declares, and record
//! exactly what was installed.
//!
//! One job, four slices, each owned by one submodule:
//!   - `manifest`  — what the project asked for: `cook.toml`'s `[modules]` and
//!                   `[registry].indexes`.
//!   - `lockfile`  — what it got: `cook.lock`, its integrity digests, and the
//!                   closure introspection that produces them.
//!   - `driver`    — how it gets it: the `~/.cook/bin/luarocks` subprocess.
//!   - `cli`       — the `cook modules` subcommand, which consumes the three.
//!
//! Shared invariant: `BTreeMap`/`BTreeSet` for any serialised collection, so
//! two runs that installed the same closure write the same bytes.
//!
//! This crate reaches the rest of the workspace through `cook-contracts` and
//! nothing else. It has no idea what a recipe, a work unit, a cache key or a
//! Lua VM is, and the day it needs one of them is the day this boundary was
//! drawn in the wrong place. See `README.md` for the charter.

pub mod cli;
pub mod driver;
pub mod lockfile;
pub mod manifest;

pub use cli::{run, ModulesArgs};
