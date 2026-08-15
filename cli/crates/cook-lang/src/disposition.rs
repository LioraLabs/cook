//! Cook-step cache modifier parsing (§8.4.3, COOK-171).
//!
//! The v3 `disposition_line` / `disposition_block` decorator grammar was
//! collapsed (COOK-171) into two crisp surfaces:
//!
//! * `seal` as a **recipe-body step** — a second input stream
//!   declaring *determinant* probe inputs. The recipe-level baseline is
//!   parsed in `recipe.rs`; this module only validates the refs
//!   ([`parse_seal_refs`]).
//! * **Trailing `cook_mods`** on a `cook` step — `share_mod?`, parsed by
//!   [`parse_cook_modifiers`]. `share_mod` is the one
//!   trailing slot collapsing `local` / `pinned` / `nondet` (mutual exclusion
//!   grammar-enforced).
//!
//! The third `share_mod` value is `nondet` (the renamed v3 `record`
//! disposition): a *fact* declaration that the output is non-reproducible.
//! Internally it still maps to the `Disposition.record` boolean — no semantic
//! change to the v3 key model.

use crate::ParseError;

/// Trailing `cook_mods` parsed off a `cook` step's tail (App. A.4 §A.4).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CookModifiers {
    /// `local` / `pinned` sharing (default `Shared`).
    pub sharing: cook_contracts::Sharing,
    /// `nondet` — the renamed v3 `record` disposition.
    pub record: bool,
}

/// Parse a `cook` step's trailing modifier tail:
///
/// ```text
/// cook_mods ::= share_mod?
/// share_mod ::= "local" | "pinned" | "nondet"
/// ```
///
/// `tail` is the whitespace-trimmed text following the step (after the body's
/// closing `}`, or after the output patterns for a declaration-only cook).
/// An empty tail yields the default modifiers. `share_mod` is a single optional
/// slot; removed keywords retain migration diagnostics.
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
            "seal" => return Err(removed_trailing_seal("cook", line)),
            "unseal" => return Err(removed_unseal(line)),
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

pub(crate) fn removed_trailing_seal(step: &str, line: usize) -> ParseError {
    ParseError::Parse {
        line,
        message: format!("{step}: `seal` was removed as a trailing modifier (CS-0225); use a recipe-level `seal` step"),
    }
}

pub(crate) fn removed_unseal(line: usize) -> ParseError {
    ParseError::Parse {
        line,
        message: "`unseal` was removed (CS-0225); do not put the ref in the recipe's `seal` step".to_string(),
    }
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

pub(crate) fn parse_seal_ref_text(text: &str, line: usize) -> Result<Vec<String>, ParseError> {
    let mut refs = Vec::new();
    let mut start = None;
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in text.char_indices() {
        if start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            start = Some(i);
        }
        if quoted && escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch.is_whitespace() && !quoted {
            refs.push(text[start.take().unwrap()..i].to_string());
        }
    }
    if let Some(start) = start {
        refs.push(text[start..].to_string());
    }
    parse_seal_refs(&refs, line)
}

#[cfg(test)]
#[path = "tests/disposition_tests.rs"]
mod tests;
