//! Disposition decorator parsing for recipe bodies (§8.4).
//!
//! A *disposition* tunes how a recipe step's cache entry is shared, recorded,
//! and sealed. The surface syntax is a set of decorator lines (`local`,
//! `pinned`, `record`, `seal …`) and their block-opener forms (`local {`,
//! `pinned {`, `record {`, `seal … {`). This module provides:
//!
//! * [`classify_disposition_line`] — recognise a body Content line as a
//!   disposition decorator/opener, or `None` if it is an ordinary step.
//! * [`parse_seal_refs`] — validate + collect bare `BARE_PROBE_KEY` refs.
//! * additive/override fold helpers ([`apply_seal`], [`apply_record`],
//!   [`apply_local`], [`apply_pinned`]).

use crate::ast::Disposition;
use crate::ParseError;

/// One recognised disposition line/opener inside a recipe body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispLine {
    Local,
    Pinned,
    Record,
    Seal(Vec<String>),
    SealBlockOpen(Vec<String>),
    LocalBlockOpen,
    PinnedBlockOpen,
    RecordBlockOpen,
}

/// First token of `text` (already trimmed) up to whitespace or `{`.
fn first_word(text: &str) -> &str {
    let end = text
        .find(|c: char| c.is_whitespace() || c == '{')
        .unwrap_or(text.len());
    &text[..end]
}

/// Classify a recipe-body Content line as a disposition line/opener, or
/// `None` if it is not one (then it dispatches per normal step priority).
/// A `local`/`pinned`/`record` keyword followed by anything other than `{`
/// (after trimming) is NOT a disposition line (returns None) — it falls
/// through to shell_command, preserving non-reserved-word status.
/// `seal` always matches (its refs may be empty); the refs are passed
/// through verbatim (including any quote chars) for `parse_seal_refs` to
/// validate at apply-time.
pub(crate) fn classify_disposition_line(text: &str) -> Option<DispLine> {
    let text = text.trim();
    let kw = first_word(text);
    let rest_full = text[kw.len()..].trim_start();
    let (rest, is_block) = match rest_full.strip_suffix('{') {
        Some(r) => (r.trim_end(), true),
        None => (rest_full, false),
    };
    match kw {
        "local" if rest.is_empty() => Some(if is_block {
            DispLine::LocalBlockOpen
        } else {
            DispLine::Local
        }),
        "pinned" if rest.is_empty() => Some(if is_block {
            DispLine::PinnedBlockOpen
        } else {
            DispLine::Pinned
        }),
        "record" if rest.is_empty() => Some(if is_block {
            DispLine::RecordBlockOpen
        } else {
            DispLine::Record
        }),
        "seal" => {
            let refs: Vec<String> = rest.split_whitespace().map(|s| s.to_string()).collect();
            Some(if is_block {
                DispLine::SealBlockOpen(refs)
            } else {
                DispLine::Seal(refs)
            })
        }
        _ => None,
    }
}

/// Validate + collect bare `BARE_PROBE_KEY` refs (`IDENT (":" IDENT)?`).
/// Rejects empty idents, a third `:IDENT` segment, and the quoted form.
/// `refs` is the already-split list of ref tokens.
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

/// Union `refs` into `d.seal`. If `d` is already `local`/`pinned` (unshared),
/// seal refs are inert and dropped.
pub(crate) fn apply_seal(d: &mut Disposition, refs: &[String]) {
    if d.sharing != cook_contracts::Sharing::Shared {
        return;
    }
    for r in refs {
        d.seal.insert(r.clone());
    }
}

pub(crate) fn apply_record(d: &mut Disposition) {
    d.record = true;
}

/// Set `local`; mutually exclusive with `pinned`. `local` drops inherited
/// seal refs (§8.4.3 override rule). The mutual-exclusion diagnostic is
/// preserved: an author who already wrote `pinned` gets the same error.
pub(crate) fn apply_local(d: &mut Disposition, line: usize) -> Result<(), ParseError> {
    if d.sharing == cook_contracts::Sharing::Pinned {
        return Err(ParseError::Parse {
            line,
            message: "disposition: `local` and `pinned` are mutually exclusive".into(),
        });
    }
    d.sharing = cook_contracts::Sharing::Local;
    d.seal.clear();
    Ok(())
}

/// Set `pinned`; mutually exclusive with `local`. Drops inherited seal refs.
pub(crate) fn apply_pinned(d: &mut Disposition, line: usize) -> Result<(), ParseError> {
    if d.sharing == cook_contracts::Sharing::Local {
        return Err(ParseError::Parse {
            line,
            message: "disposition: `local` and `pinned` are mutually exclusive".into(),
        });
    }
    d.sharing = cook_contracts::Sharing::Pinned;
    d.seal.clear();
    Ok(())
}

/// Fold one recognised non-block disposition decorator (`DispLine::Local`,
/// `Pinned`, `Record`, or `Seal`) into the pending disposition `d`. Used by
/// both the recipe-body loop and the disposition-block parser so the
/// decorator-accumulation rules live in one place. Block-opener variants are
/// the caller's concern (they start a nested block) and are rejected here.
pub(crate) fn fold_decorator(
    d: &mut Disposition,
    disp: &DispLine,
    line: usize,
) -> Result<(), ParseError> {
    match disp {
        DispLine::Local => apply_local(d, line),
        DispLine::Pinned => apply_pinned(d, line),
        DispLine::Record => {
            apply_record(d);
            Ok(())
        }
        DispLine::Seal(refs) => {
            let refs = parse_seal_refs(refs, line)?;
            apply_seal(d, &refs);
            Ok(())
        }
        DispLine::SealBlockOpen(_)
        | DispLine::LocalBlockOpen
        | DispLine::PinnedBlockOpen
        | DispLine::RecordBlockOpen => Err(ParseError::Parse {
            line,
            message: "disposition: a block opener cannot be a decorator line".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bare_keywords() {
        assert_eq!(classify_disposition_line("local"), Some(DispLine::Local));
        assert_eq!(classify_disposition_line("pinned"), Some(DispLine::Pinned));
        assert_eq!(classify_disposition_line("record"), Some(DispLine::Record));
        assert_eq!(
            classify_disposition_line("seal"),
            Some(DispLine::Seal(vec![]))
        );
        // surrounding whitespace tolerated
        assert_eq!(
            classify_disposition_line("   local   "),
            Some(DispLine::Local)
        );
    }

    #[test]
    fn classify_block_openers() {
        assert_eq!(
            classify_disposition_line("local {"),
            Some(DispLine::LocalBlockOpen)
        );
        assert_eq!(
            classify_disposition_line("pinned {"),
            Some(DispLine::PinnedBlockOpen)
        );
        assert_eq!(
            classify_disposition_line("record {"),
            Some(DispLine::RecordBlockOpen)
        );
        // no space before brace still opens a block
        assert_eq!(
            classify_disposition_line("local{"),
            Some(DispLine::LocalBlockOpen)
        );
        assert_eq!(
            classify_disposition_line("seal host gpu {"),
            Some(DispLine::SealBlockOpen(vec![
                "host".to_string(),
                "gpu".to_string()
            ]))
        );
        // seal block with no refs
        assert_eq!(
            classify_disposition_line("seal {"),
            Some(DispLine::SealBlockOpen(vec![]))
        );
    }

    #[test]
    fn classify_seal_collects_refs() {
        assert_eq!(
            classify_disposition_line("seal host gpu cc:toolchain"),
            Some(DispLine::Seal(vec![
                "host".to_string(),
                "gpu".to_string(),
                "cc:toolchain".to_string()
            ]))
        );
    }

    #[test]
    fn classify_returns_none_for_non_disposition() {
        // a cook step with quoted output + block
        assert_eq!(classify_disposition_line("cook \"x\" {"), None);
        // not a reserved word
        assert_eq!(classify_disposition_line("sealant foo"), None);
        assert_eq!(classify_disposition_line("locally"), None);
        // local/pinned/record with trailing non-block content fall through
        assert_eq!(classify_disposition_line("local foo"), None);
        assert_eq!(classify_disposition_line("record x"), None);
        assert_eq!(classify_disposition_line("pinned something"), None);
    }

    #[test]
    fn parse_seal_refs_accepts_bare_keys() {
        let refs = vec!["host".to_string(), "cc:toolchain".to_string()];
        let out = parse_seal_refs(&refs, 3).expect("should accept bare keys");
        assert_eq!(out, vec!["host".to_string(), "cc:toolchain".to_string()]);
        // underscore-led and digit-containing idents
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
        // an empty token, and ones with empty segments
        assert!(parse_seal_refs(&["".to_string()], 1).is_err());
        assert!(parse_seal_refs(&["a:".to_string()], 1).is_err());
        assert!(parse_seal_refs(&[":b".to_string()], 1).is_err());
    }

    #[test]
    fn apply_seal_is_additive_and_sorted() {
        let mut d = Disposition::default();
        apply_seal(&mut d, &["gpu".to_string(), "host".to_string()]);
        apply_seal(&mut d, &["host".to_string(), "cc".to_string()]);
        // union, deduped, sorted by BTreeSet iteration order
        let got: Vec<String> = d.seal.iter().cloned().collect();
        assert_eq!(
            got,
            vec!["cc".to_string(), "gpu".to_string(), "host".to_string()]
        );
    }

    #[test]
    fn apply_local_sets_and_clears_seal() {
        let mut d = Disposition::default();
        apply_seal(&mut d, &["host".to_string()]);
        apply_local(&mut d, 1).expect("local should succeed");
        assert_eq!(d.sharing, cook_contracts::Sharing::Local);
        assert!(d.seal.is_empty());
    }

    #[test]
    fn apply_pinned_after_local_errors() {
        let mut d = Disposition::default();
        apply_local(&mut d, 1).unwrap();
        assert!(apply_pinned(&mut d, 2).is_err());
    }

    #[test]
    fn apply_local_after_pinned_errors() {
        let mut d = Disposition::default();
        apply_pinned(&mut d, 1).unwrap();
        assert!(apply_local(&mut d, 2).is_err());
    }

    #[test]
    fn apply_seal_after_local_is_noop() {
        let mut d = Disposition::default();
        apply_local(&mut d, 1).unwrap();
        apply_seal(&mut d, &["host".to_string()]);
        assert!(d.seal.is_empty());
    }

    #[test]
    fn apply_seal_after_pinned_is_noop() {
        let mut d = Disposition::default();
        apply_pinned(&mut d, 1).unwrap();
        apply_seal(&mut d, &["host".to_string()]);
        assert!(d.seal.is_empty());
    }

    #[test]
    fn apply_record_sets_flag() {
        let mut d = Disposition::default();
        apply_record(&mut d);
        assert!(d.record);
    }
}
