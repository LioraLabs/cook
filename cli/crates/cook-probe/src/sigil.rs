//! `$<key:field[i]>` substitution over a probe's materialised value
//! (§22.5.7, CS-0188, CS-0192).
//!
//! No VM is in the loop, which is why this lives beside the store rather
//! than in either phase's Lua host: a probe reference in a shell command is
//! resolved by reading bytes and rendering them, and the phase that happens
//! to be spawning the command is not part of the answer.

use crate::store::{not_materialised_message, ProbeValueStore};

/// Substitute `$<key:field[i]>` probe references in a command with their
/// resolved values, immediately before the command is spawned (CS-0188).
///
/// Before CS-0188 this happened by rewriting the command, at register phase,
/// into a Lua chunk that read each value and called `cook.sh` with the result.
/// The rewrite is gone; the substitution is not, because §22.5.7 still requires
/// it and a probe's value is execute-phase, so that is the only phase it can
/// happen in.
///
/// **The rendering is the Standard's, computed in Rust (CS-0192).** Each
/// reference addresses the value's read view — canonical JSON plus the
/// CS-0157 tool-path annotation, built by the same function that builds it
/// for `cook.probes.get` — and renders through `cook_contracts::sigil::subst`,
/// the one place §22.5.7's rules live. Before CS-0192 the walk read through
/// `cook.probes.get` and finished with Lua's `tostring`, under which a table
/// interpolated its heap address (different bytes on the same command line
/// every run) and an absent member the four bytes `nil`.
///
/// A command with no probe reference is returned untouched, and a non-probe
/// `$<...>` span is left literal, both matching what the rewrite did.
///
/// **What makes a span a probe reference (CS-0240).** A colon-carrying base is
/// one on sight, and an unmaterialised one is still the CS-0152 diagnostic
/// below — the colon form cannot mean anything else, so a miss is a miss and
/// not a maybe. A colon-free base is one when the store holds it, which is
/// §22.5.7's "classify by membership in the unit's `probes` list" read off the
/// thing that list produced: codegen puts exactly the keys it resolved into
/// that list, and the store is populated from it before the command is spawned.
/// A colon-free base the store does not hold is left literal, because by this
/// point every non-probe sigil has already been substituted at register time —
/// what is left is text the shell owns.
pub fn resolve_probe_sigils(store: &ProbeValueStore, cmd: &str) -> Result<String, String> {
    let spans = cook_contracts::sigil::scan(cmd);
    if spans.is_empty() {
        return Ok(cmd.to_string());
    }
    let materialised = |key: &str| store.get(key).is_some();
    let refs: Vec<_> = spans
        .iter()
        .filter_map(|s| cook_contracts::sigil::probe_ref(&s.ident, materialised).map(|r| (s, r)))
        .collect();
    if refs.is_empty() {
        return Ok(cmd.to_string());
    }

    let mut out = String::with_capacity(cmd.len());
    let mut cursor = 0usize;
    for (span, r) in refs {
        out.push_str(&cmd[cursor..span.range.start]);

        // An unmaterialised key is the CS-0152 diagnostic, the same text
        // `cook.probes.get` raises for the same miss.
        let bytes = store
            .get(r.key())
            .ok_or_else(|| not_materialised_message(r.key()))?;
        let value = store
            .read_view(r.key(), &bytes)
            .map_err(|e| format!("$<{}>: probe value decode failed: {e}", span.ident))?;
        out.push_str(&cook_contracts::sigil::subst::substitute(
            &value,
            r.path(),
            &span.ident,
        )?);
        cursor = span.range.end;
    }
    out.push_str(&cmd[cursor..]);
    Ok(out)
}

#[cfg(test)]
#[path = "tests/sigil_tests.rs"]
mod tests;
