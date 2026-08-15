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

use crate::ast::{Probe, ProbeProduce};
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

/// CS-0226, CS-0235: the one removed-`envs` diagnostic, for both positions the
/// keyword used to occupy — the probe producer (§22.5.2) and the recipe body
/// (§8.1 cascade rule 3). `spelling` is the form the author wrote, since the two
/// positions spell the construct differently (`envs { A, B }` vs `envs A B`);
/// everything the diagnostic has to get RIGHT — the change ID and the
/// replacement rendered from the author's own names — is shared, so the two
/// cannot drift into saying different things about one removal.
///
/// An empty `names` falls back to the Standard's own example, `echo "$NAME"`
/// (§22.5.2), which is also what the operand list renders to when it cannot be
/// parsed at all.
pub(crate) fn removed_envs(spelling: &str, names: &[String], line: usize) -> ParseError {
    let replacement = if names.is_empty() {
        "echo \"$NAME\"".to_string()
    } else {
        names
            .iter()
            .map(|name| format!("echo \"${name}\""))
            .collect::<Vec<_>>()
            .join("; ")
    };
    ParseError::Parse {
        line,
        message: format!(
            "`{spelling}` was removed (CS-0226); use an ordinary shell probe: \
             `lines {{ {replacement} }}`"
        ),
    }
}

/// Validate bare probe-key operands for `seal`.
pub(crate) fn parse_seal_refs(refs: &[String], line: usize) -> Result<Vec<String>, ParseError> {
    let mut out = Vec::new();
    for tok in refs {
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

pub(crate) struct SealOperands {
    pub refs: Vec<String>,
    pub inline_probe: Option<Probe>,
}

/// The reserved key for one anonymous file determinant (CS-0236).
///
/// Derived from the operands and nothing else: §17.4 forbids the recipe name
/// and the step's source position from a cache key or a unit identity, and this
/// key is folded into the `seal_contribution` of *every* cacheable unit in the
/// owning recipe (§8.4.3.1 rule 1). Keying it by owner and line therefore made
/// renaming a recipe, or moving any line above a `seal`, a cache miss for every
/// unit in it.
///
/// The operands are the whole declaration — an anonymous probe's producer is
/// the files-manifest sentinel and its inputs are these globs — so equal keys
/// hold exactly when the declarations are equal, and two identical seals are
/// one determinant rather than a collision (`parse` de-duplicates them).
///
/// Spelled as a hex fold rather than the globs verbatim because a probe key is
/// `:`-segmented and `.`-qualified by the import prefix, and a glob carries
/// `/ * . :`.
pub(crate) fn anonymous_seal_key(globs: &[String], excludes: &[String]) -> String {
    let mut records: Vec<String> = globs
        .iter()
        .map(|g| format!("+{g}"))
        .chain(excludes.iter().map(|e| format!("-{e}")))
        .collect();
    records.sort();
    records.dedup();
    format!(
        "@seal:{:016x}",
        cook_contracts::hash::hash_str(&records.join("\n"))
    )
}

pub(crate) fn parse_seal_operands(
    text: &str,
    line: usize,
) -> Result<SealOperands, ParseError> {
    let mut operands = Vec::new();
    let mut start = None;
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in text.char_indices() {
        if start.is_none() {
            if ch.is_whitespace() { continue; }
            start = Some(i);
        }
        if quoted && escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch.is_whitespace() && !quoted {
            operands.push(text[start.take().unwrap()..i].to_string());
        }
    }
    if let Some(start) = start {
        operands.push(text[start..].to_string());
    }
    if quoted {
        return Err(ParseError::Parse {
            line,
            message: "seal: unterminated quoted file glob".into(),
        });
    }

    let mut refs = Vec::new();
    let mut globs = Vec::new();
    let mut excludes = Vec::new();
    for operand in operands {
        if let Some(quoted) = operand.strip_prefix('!') {
            let inner = quoted
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or_else(|| ParseError::Parse {
                    line,
                    message: "seal: `!` must be immediately followed by a quoted glob".into(),
                })?;
            if inner.is_empty() {
                return Err(ParseError::Parse {
                    line,
                    message: "seal: excluded file glob must not be empty".into(),
                });
            }
            excludes.push(inner.to_string());
        } else if let Some(inner) = operand.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            if inner.is_empty() {
                return Err(ParseError::Parse {
                    line,
                    message: "seal: file glob must not be empty".into(),
                });
            }
            globs.push(inner.to_string());
        } else {
            refs.extend(parse_seal_refs(&[operand], line)?);
        }
    }
    if globs.is_empty() && !excludes.is_empty() {
        return Err(ParseError::Parse {
            line,
            // CS-0238: the rule is asked over the STEP, which may span
            // continuation lines, not over one physical line.
            message: "seal: an excluded glob requires a quoted include glob in the same `seal` step"
                .into(),
        });
    }
    let inline_probe = if globs.is_empty() { None } else {
        let name = anonymous_seal_key(&globs, &excludes);
        refs.push(name.clone());
        Some(Probe {
            name,
            deps: vec![],
            inputs: vec![],
            excludes: vec![],
            produce: ProbeProduce::Files { globs, excludes },
            line,
        })
    };
    Ok(SealOperands { refs, inline_probe })
}

#[cfg(test)]
#[path = "tests/disposition_tests.rs"]
mod tests;
