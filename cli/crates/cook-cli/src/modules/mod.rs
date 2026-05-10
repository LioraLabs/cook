//! `cook modules` — manifest, lockfile, and LuaRocks subprocess driver.
//!
//! Phase 3 surface (SHI-176). Each submodule is owned by one slice:
//!   - `manifest`  — M3.1: cook.toml `[modules]` and `[registry].indexes` parsing.
//!   - `lockfile`  — M3.2: cook.lock format, integrity verification, closure introspection.
//!   - `driver`    — M3.3: `~/.cook/bin/luarocks` subprocess wrapper.
//!   - `cli`       — M3.4: clap subcommand wiring; consumes the three above.
//!
//! Shared invariant: `BTreeMap`/`BTreeSet` for any serialised collection
//! (deterministic output per project conventions).

pub mod cli;
pub mod driver;
pub mod lockfile;
pub mod manifest;

pub use cli::{run, ModulesArgs};
