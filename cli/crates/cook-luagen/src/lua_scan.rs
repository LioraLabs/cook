//! The free-identifier walk over Lua source.
//!
//! Two callers in this crate ask the same question of a Lua body and must get
//! the same answer: [`crate::template`] asks whether a plate/test body reads
//! `input` or `inputs`, and [`crate::use_prelude`] asks whether a body names a
//! `use` alias. Each carried its own copy of the walk (COOK-357).
//!
//! Where a string or comment begins and ends is NOT decided here. That is one
//! lexical fact about Lua with a consumer in another crate as well, so it lives
//! in [`cook_contracts::lua_scan`] and this module asks it (COOK-403). What is
//! left here is the part that is genuinely this crate's: what counts as a free
//! identifier once the non-code regions are out of the way.

use cook_contracts::lua_scan::{ident_end, is_ident_start, skip_non_code, Skip};

/// Does `ident` occur as a FREE identifier — a read of a name in scope, rather
/// than a field or method of something else — in the code regions of `src`?
///
/// This is the crate's one free-identifier walk. Two callers ask the same
/// question of a Lua body and must get the same answer:
///
/// - [`crate::template`] asks whether a plate/test body reads `input` or
///   `inputs`, which picks the body's iteration mode;
/// - [`crate::use_prelude`] asks whether a body names a `use` alias, which
///   decides whether the CS-0205 binding is prepended.
///
/// Both directions of a wrong answer are defects, and they are asymmetric.
/// A missed reference (false negative) is the defect CS-0205 fixes: the alias
/// is unbound and the body dies on a nil global. A spurious one (false
/// positive) is NOT the harmless extra module load it looks like — the binding
/// evaluates the module's top level and `init()` on a worker VM for a body that
/// never asked, which raises for a module written for register phase, and under
/// CS-0204 it puts that module in the unit's cache key.
///
/// So both are ruled out where they actually arise:
///
/// - a whole token is consumed at once, so `mygreet` and `greet2` cannot
///   partial-match `greet`;
/// - a name immediately behind `.` or `:` is a field or method (`t.greet`),
///   not this scope's `greet` — unless that dot is the second of a `..`
///   concatenation (`"pre"..greet.f()`), where the name IS free. Checking only
///   the single preceding byte gets that case wrong in the false-negative
///   direction.
///
/// `load`ed chunks and `_ENV` lookups are deliberately not considered: neither
/// can see a `local`, so neither can observe the binding.
pub(crate) fn free_identifier_occurs(src: &str, ident: &str) -> bool {
    if ident.is_empty() {
        return false;
    }
    let bytes = src.as_bytes();
    let needle = ident.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match skip_non_code(src, i) {
            Skip::Ended(next) => i = next.max(i + 1),
            // Past an unterminated literal there is no honest answer; guessing
            // where it ends invents matches.
            Skip::Unterminated => return false,
            Skip::Code => {
                if is_ident_start(bytes[i]) {
                    let start = i;
                    let end = ident_end(bytes, i);
                    if &bytes[start..end] == needle && !is_field_access_at(bytes, start) {
                        return true;
                    }
                    i = end;
                } else {
                    i += 1;
                }
            }
        }
    }
    false
}

/// Is the identifier starting at `start` the field or method half of an access?
///
/// True for `t.name` and `t:name`. False for `x..name`, where the byte before
/// is a dot but is the second half of the concatenation operator.
fn is_field_access_at(bytes: &[u8], start: usize) -> bool {
    if start == 0 {
        return false;
    }
    match bytes[start - 1] {
        b':' => true,
        b'.' => start < 2 || bytes[start - 2] != b'.',
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests/lua_scan_tests.rs"]
mod tests;
