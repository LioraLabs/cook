//! Cook-step cache modifier parsing (§8.4.3, COOK-171).
//!
//! The v3 `disposition_line` / `disposition_block` decorator grammar was
//! collapsed (COOK-171) into two crisp surfaces:
//!
//! * `seal` / `unseal` as **recipe-body steps** — a second input stream
//!   declaring *determinant* probe inputs. The recipe-level baseline is
//!   parsed in `recipe.rs`; this module only validates the refs
//!   ([`parse_seal_refs`]).
//! * **Trailing `cook_mods`** on a `cook` step — `(seal R+ | unseal R+)*
//!   share_mod?`, parsed by [`parse_cook_modifiers`]. `share_mod` is the one
//!   trailing slot collapsing `local` / `pinned` / `nondet` (mutual exclusion
//!   grammar-enforced).
//!
//! The third `share_mod` value is `nondet` (the renamed v3 `record`
//! disposition): a *fact* declaration that the output is non-reproducible.
//! Internally it still maps to the `Disposition.record` boolean — no semantic
//! change to the v3 key model.

use std::collections::BTreeSet;

use crate::ParseError;

/// Trailing `cook_mods` parsed off a `cook` step's tail (App. A.4 §A.4).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CookModifiers {
    /// Per-unit trailing `seal` refs (added to the recipe baseline).
    pub seal: BTreeSet<String>,
    /// Per-unit trailing `unseal` refs (removed from the effective set).
    pub unseal: BTreeSet<String>,
    /// `local` / `pinned` sharing (default `Shared`).
    pub sharing: cook_contracts::Sharing,
    /// `nondet` — the renamed v3 `record` disposition.
    pub record: bool,
}

const SHARE_MODS: [&str; 3] = ["local", "pinned", "nondet"];

/// A token that terminates a `seal`/`unseal` ref run (the next clause keyword).
fn is_clause_kw(t: &str) -> bool {
    t == "seal" || t == "unseal" || SHARE_MODS.contains(&t)
}

/// Parse a `cook` step's trailing modifier tail:
///
/// ```text
/// cook_mods ::= ("seal" probe_ref+ | "unseal" probe_ref+)* share_mod?
/// share_mod ::= "local" | "pinned" | "nondet"
/// ```
///
/// `tail` is the whitespace-trimmed text following the step (after the body's
/// closing `}`, or after the output patterns for a declaration-only cook).
/// An empty tail yields the default modifiers. `share_mod` is a single optional
/// slot that MUST be last; a bare `seal`/`unseal` (no refs) is rejected; the
/// removed `record` keyword and the removed `as` keyword get migration hints.
pub(crate) fn parse_cook_modifiers(tail: &str, line: usize) -> Result<CookModifiers, ParseError> {
    let toks: Vec<&str> = tail.split_whitespace().collect();
    let mut m = CookModifiers::default();
    let mut share_set = false;
    let mut i = 0;
    while i < toks.len() {
        if share_set {
            return Err(ParseError::Parse {
                line,
                message: "cook: no modifier may follow the disposition (share_mod must be last)"
                    .to_string(),
            });
        }
        match toks[i] {
            kw @ ("seal" | "unseal") => {
                let mut refs = Vec::new();
                i += 1;
                while i < toks.len() && !is_clause_kw(toks[i]) {
                    refs.push(toks[i].to_string());
                    i += 1;
                }
                if refs.is_empty() {
                    return Err(ParseError::Parse {
                        line,
                        message: format!(
                            "cook: `{kw}` requires at least one probe ref (bare `{kw}` is rejected)"
                        ),
                    });
                }
                let validated = parse_seal_refs(&refs, line)?;
                let dst = if kw == "seal" { &mut m.seal } else { &mut m.unseal };
                for r in validated {
                    dst.insert(r);
                }
            }
            "local" => {
                m.sharing = cook_contracts::Sharing::Local;
                share_set = true;
                i += 1;
            }
            "pinned" => {
                m.sharing = cook_contracts::Sharing::Pinned;
                share_set = true;
                i += 1;
            }
            "nondet" => {
                m.record = true;
                share_set = true;
                i += 1;
            }
            "record" => {
                return Err(ParseError::Parse {
                    line,
                    message: "cook: the `record` disposition was renamed to `nondet` \
                              (Cache-surface ergonomics, CS-0115)"
                        .to_string(),
                });
            }
            "as" => {
                return Err(ParseError::Parse {
                    line,
                    message: "cook: `as` was removed in v1.0 — it is no longer a step modifier \
                              (CS-0135)"
                        .to_string(),
                });
            }
            other => {
                return Err(ParseError::Parse {
                    line,
                    message: format!("cook: unexpected modifier `{other}`"),
                });
            }
        }
    }
    Ok(m)
}

/// Trailing `test_mods` parsed off a `test` step's tail (§8.4.3.2, CS-0159).
/// The input half of [`CookModifiers`] — a test seals and unseals, but carries
/// no `share_mod`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TestModifiers {
    pub seal: BTreeSet<String>,
    pub unseal: BTreeSet<String>,
}

/// Parse a `test` step's trailing modifier tail:
///
/// ```text
/// test_mods ::= ("seal" probe_ref+ | "unseal" probe_ref+)*
/// ```
///
/// CS-0159: a `test` unit is a cacheable unit, so it takes the *input* half of
/// `cook_mods` — `seal` / `unseal`. It takes no `share_mod`: `local` /
/// `pinned` / `nondet` are facts about an output artifact, and a test produces
/// a pass/fail record, so those keywords are rejected here with a diagnostic
/// naming the reason rather than the generic unexpected-modifier error.
///
/// The removed v1.0 modifiers (`should_fail` / `timeout` / `as`, CS-0135) keep
/// their did-you-mean migration diagnostics — this parser is the sole trailing
/// surface for `test`, so it subsumes the former reject-everything path.
pub(crate) fn parse_test_modifiers(tail: &str, line: usize) -> Result<TestModifiers, ParseError> {
    let toks: Vec<&str> = tail.split_whitespace().collect();
    let mut m = TestModifiers::default();
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            kw @ ("seal" | "unseal") => {
                let mut refs = Vec::new();
                i += 1;
                while i < toks.len() && !is_clause_kw(toks[i]) {
                    refs.push(toks[i].to_string());
                    i += 1;
                }
                if refs.is_empty() {
                    return Err(ParseError::Parse {
                        line,
                        message: format!(
                            "test: `{kw}` requires at least one probe ref (bare `{kw}` is rejected)"
                        ),
                    });
                }
                let validated = parse_seal_refs(&refs, line)?;
                let dst = if kw == "seal" { &mut m.seal } else { &mut m.unseal };
                for r in validated {
                    dst.insert(r);
                }
            }
            share @ ("local" | "pinned" | "nondet") => {
                return Err(ParseError::Parse {
                    line,
                    message: format!(
                        "test: `{share}` is not a test modifier — `local`/`pinned`/`nondet` state \
                         a fact about an output artifact, and a test produces a pass/fail result \
                         rather than artifacts (CS-0159). A test tail takes `seal`/`unseal` only."
                    ),
                });
            }
            "record" => {
                return Err(ParseError::Parse {
                    line,
                    message: "test: the `record` disposition was renamed to `nondet` (CS-0115), \
                              and neither applies to a test step (CS-0159)"
                        .to_string(),
                });
            }
            "should_fail" => {
                return Err(ParseError::Parse {
                    line,
                    message: "test: should_fail was removed in v1.0 — invert the check in the \
                              body instead (e.g. run the command and fail if it unexpectedly \
                              succeeds)"
                        .to_string(),
                });
            }
            "timeout" => {
                return Err(ParseError::Parse {
                    line,
                    message: "test: timeout was removed in v1.0 — enforce a deadline from inside \
                              the test body instead (e.g. `timeout N ...` in a shell body)"
                        .to_string(),
                });
            }
            "as" => {
                return Err(ParseError::Parse {
                    line,
                    message: "test: as was removed in v1.0 — test steps no longer take a custom \
                              name"
                        .to_string(),
                });
            }
            other => {
                return Err(ParseError::Parse {
                    line,
                    message: format!("unexpected text after test body: '{other}'"),
                });
            }
        }
    }
    Ok(m)
}

/// Validate + collect probe key refs for `seal`.
///
/// CS-0201: accepts both spellings a probe key has. A bare ref must match
/// `cook_contracts::probe_key` (one or more `:`-separated
/// `[A-Za-z_][A-Za-z0-9_-]*` segments); a quoted ref is any non-empty string
/// and is the escape hatch.
///
/// What this replaced: `seal` alone allowed neither `-` nor `.`, capped at two
/// segments, and refused the quoted form outright, while the declaration that
/// mints the key allowed `-`, `.` and quoting. So `probe cc-version` produced
/// a key that could not be sealed, and `cc:find:raylib` — three segments, and
/// the flagship module's ordinary case — could not be sealed either, which is
/// precisely the pin a cache-trust story exists to offer.
pub(crate) fn parse_seal_refs(refs: &[String], line: usize) -> Result<Vec<String>, ParseError> {
    let mut out = Vec::new();
    for tok in refs {
        // The quoted form: strip the delimiters and take the contents as-is.
        if let Some(inner) = tok.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
            if inner.is_empty() {
                return Err(ParseError::Parse {
                    line,
                    message: "seal: probe key must not be empty".to_string(),
                });
            }
            out.push(inner.to_string());
            continue;
        }
        if !cook_contracts::probe_key::is_valid_bare(tok) {
            return Err(ParseError::Parse {
                line,
                message: cook_contracts::probe_key::bare_key_error("seal", tok),
            });
        }
        out.push(tok.clone());
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tests/disposition_tests.rs"]
mod tests;
