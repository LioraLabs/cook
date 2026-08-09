//! Reading a Lua body's text for what it declares.
//!
//! Two questions, one walk. [`scan`] skips the regions of Lua source that are
//! not code — strings and comments — and [`reads`] uses that walk to answer
//! what a body reads: which `var.NAME` values, and which probe keys it fetches
//! through `cook.probes.get("k")`.
//!
//! This is grammar, which the constitution admits by name, and it is answered
//! in more than one phase. Codegen folds the scanned `var` names into a unit's
//! `consulted_env_keys`, and the register phase asks the same source the same
//! question about probe keys to decide which probes a unit declares. Those two
//! must agree: a key one sees and the other does not is a probe read that
//! never became a determinant.
//!
//! It lived in `cook-luagen` until COOK-421, which made `cook-register` — the
//! phase that RUNS generated Lua — depend on the crate that GENERATES it, for
//! a scanner that generates nothing. The scanner was the last production
//! reason for that edge, and the edge is gone with it.

mod reads;
mod scan;

pub use reads::{scan_probe_reads, scan_var_reads};
pub use scan::{free_identifier_occurs, ident_end, is_ident_cont, is_ident_start, skip_non_code, Skip};
