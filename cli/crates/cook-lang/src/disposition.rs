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

/// Validate + collect bare `BARE_PROBE_KEY` refs (`IDENT (":" IDENT)?`).
/// Rejects empty idents, a third `:IDENT` segment, and the quoted form.
/// `refs` is the already-split list of ref tokens. Shared by the recipe-level
/// `seal` step (recipe.rs) and the trailing `cook_mods` parser above.
pub(crate) fn parse_seal_refs(refs: &[String], line: usize) -> Result<Vec<String>, ParseError> {
    let mut out = Vec::new();
    for tok in refs {
        if tok.starts_with('"') {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "seal: probe ref must be a bare key (IDENT[:IDENT]), not the quoted form: {tok}"
                ),
            });
        }
        let segs: Vec<&str> = tok.split(':').collect();
        let ok = (segs.len() == 1 || segs.len() == 2)
            && segs.iter().all(|s| {
                !s.is_empty()
                    && s.chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });
        if !ok {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "seal: malformed probe ref '{tok}' (expected IDENT or IDENT:IDENT)"
                ),
            });
        }
        out.push(tok.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods_empty_tail_is_default() {
        let m = parse_cook_modifiers("", 1).unwrap();
        assert!(m.seal.is_empty() && m.unseal.is_empty());
        assert_eq!(m.sharing, cook_contracts::Sharing::Shared);
        assert!(!m.record);
    }

    #[test]
    fn mods_share_mod_local_pinned_nondet() {
        assert_eq!(
            parse_cook_modifiers("local", 1).unwrap().sharing,
            cook_contracts::Sharing::Local
        );
        assert_eq!(
            parse_cook_modifiers("pinned", 1).unwrap().sharing,
            cook_contracts::Sharing::Pinned
        );
        assert!(parse_cook_modifiers("nondet", 1).unwrap().record);
    }

    #[test]
    fn mods_seal_unseal_collect_refs() {
        let m = parse_cook_modifiers("seal a b unseal c", 1).unwrap();
        assert_eq!(
            m.seal.iter().cloned().collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            m.unseal.iter().cloned().collect::<Vec<_>>(),
            vec!["c".to_string()]
        );
    }

    #[test]
    fn mods_seal_then_share_mod() {
        let m = parse_cook_modifiers("seal rev local", 1).unwrap();
        assert!(m.seal.contains("rev"));
        assert_eq!(m.sharing, cook_contracts::Sharing::Local);
    }

    #[test]
    fn mods_bare_seal_rejected() {
        assert!(parse_cook_modifiers("seal", 1).is_err());
        // `local` terminates the ref run → bare seal
        assert!(parse_cook_modifiers("seal local", 1).is_err());
        assert!(parse_cook_modifiers("unseal", 1).is_err());
    }

    #[test]
    fn mods_two_share_mods_rejected() {
        assert!(parse_cook_modifiers("local pinned", 1).is_err());
        assert!(parse_cook_modifiers("nondet local", 1).is_err());
    }

    #[test]
    fn mods_content_after_share_mod_rejected() {
        assert!(parse_cook_modifiers("local seal a", 1).is_err());
    }

    #[test]
    fn mods_record_keyword_hints_nondet() {
        let e = parse_cook_modifiers("record", 1).unwrap_err();
        if let ParseError::Parse { message, .. } = e {
            assert!(message.contains("nondet"));
        } else {
            panic!("expected Parse error");
        }
    }

    #[test]
    fn mods_as_keyword_hints_removed_in_v1() {
        let e = parse_cook_modifiers("as 'x'", 1).unwrap_err();
        if let ParseError::Parse { message, .. } = e {
            assert!(message.contains("removed in v1.0"));
        } else {
            panic!("expected Parse error");
        }
    }

    #[test]
    fn mods_seal_quoted_and_triple_colon_rejected() {
        assert!(parse_cook_modifiers("seal \"host\"", 1).is_err());
        assert!(parse_cook_modifiers("seal a:b:c", 1).is_err());
    }

    #[test]
    fn parse_seal_refs_accepts_bare_keys() {
        let refs = vec!["host".to_string(), "cc:toolchain".to_string()];
        let out = parse_seal_refs(&refs, 3).expect("should accept bare keys");
        assert_eq!(out, vec!["host".to_string(), "cc:toolchain".to_string()]);
        let refs = vec!["_x".to_string(), "a1:_b2".to_string()];
        let out = parse_seal_refs(&refs, 3).expect("should accept underscore/digit idents");
        assert_eq!(out, vec!["_x".to_string(), "a1:_b2".to_string()]);
    }

    #[test]
    fn parse_seal_refs_rejects_third_segment() {
        let refs = vec!["a:b:c".to_string()];
        let err = parse_seal_refs(&refs, 7).unwrap_err();
        match err {
            ParseError::Parse { line, message } => {
                assert_eq!(line, 7);
                assert!(message.contains("a:b:c"));
            }
            _ => panic!("expected Parse error"),
        }
    }

    #[test]
    fn parse_seal_refs_rejects_quoted_form() {
        let refs = vec!["\"host\"".to_string()];
        let err = parse_seal_refs(&refs, 2).unwrap_err();
        match err {
            ParseError::Parse { line, message } => {
                assert_eq!(line, 2);
                assert!(message.contains("quoted"));
            }
            _ => panic!("expected Parse error"),
        }
    }

    #[test]
    fn parse_seal_refs_rejects_leading_digit() {
        let refs = vec!["1bad".to_string()];
        assert!(parse_seal_refs(&refs, 1).is_err());
    }

    #[test]
    fn parse_seal_refs_rejects_empty_segment() {
        assert!(parse_seal_refs(&["".to_string()], 1).is_err());
        assert!(parse_seal_refs(&["a:".to_string()], 1).is_err());
        assert!(parse_seal_refs(&[":b".to_string()], 1).is_err());
    }
}
