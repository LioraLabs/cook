//! Probe-value substitution rendering (§22.5.7, CS-0192).
//!
//! A `$<key…>` placeholder in command text substitutes the addressed part of
//! the probe's value, and the rendering is the Standard's own, defined over
//! the value's JSON type. Before CS-0192 the rendering was Lua's `tostring`
//! by reference: a table interpolated its heap address — different bytes on
//! the same command line every run — and an absent member the four bytes
//! `nil`, a shell word that runs.
//!
//! Addressing operates on the value's READ VIEW: the canonical value with a
//! `tools` producer's per-run path annotation merged in (CS-0157, see
//! [`crate::probe_value::merge_tool_paths`]). The caller prepares that view;
//! this module is a pure function of the prepared value and the parsed path.
//!
//! Why this lives in `cook-contracts`: it is a pure function of a JSON value
//! and a [`Seg`] path, and it is law — §22.5.7 states these rules
//! normatively, and every site that substitutes a probe value into text an
//! author observes must agree on them, or one sigil means different bytes in
//! different positions (the disagreement CS-0184 forbids).

use serde_json::Value as JsonValue;

use super::Seg;

/// `"a"`/`"an"` for a JSON type name, so a diagnostic reads as a sentence.
fn article(v: &JsonValue) -> &'static str {
    match v {
        JsonValue::Array(_) | JsonValue::Object(_) => "an",
        _ => "a",
    }
}

/// The JSON type name of a value, as diagnostics spell it (§22.5.7).
pub fn json_type_name(v: &JsonValue) -> &'static str {
    match v {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

/// Walk `path` over `root` and render the addressed value for command
/// position per §22.5.7 (CS-0192). `ident` is the placeholder's IDENT
/// (`v:arr[1]`, no `$<>`), used verbatim in every diagnostic.
///
/// Rendering: a string is its content, verbatim; a number is its canonical
/// JSON token (the same bytes any hash of the value folds — asserted against
/// [`crate::probe_value::encode_canonical_json`] in this module's tests); a
/// boolean is `true`/`false`. `null`, arrays and objects are diagnostics,
/// never renderings, as is every addressing failure.
pub fn substitute(root: &JsonValue, path: &[Seg], ident: &str) -> Result<String, String> {
    let mut value = root;
    for seg in path {
        value = match seg {
            Seg::Field(name) => match value {
                JsonValue::Object(map) => map.get(name).ok_or_else(|| {
                    format!("$<{ident}>: the value has no member '{name}'")
                })?,
                other => {
                    return Err(format!(
                        "$<{ident}>: cannot address member '{name}' of {} {} value",
                        article(other),
                        json_type_name(other)
                    ))
                }
            },
            Seg::Index(idx) => {
                // §22.5.7 defines `[i]` as a one-based array element.
                let i: usize = idx.parse().map_err(|_| {
                    format!("$<{ident}>: `[{idx}]` is not a numeric index")
                })?;
                match value {
                    JsonValue::Array(items) => {
                        if i == 0 {
                            return Err(format!(
                                "$<{ident}>: `[0]` — array indices are one-based"
                            ));
                        }
                        items.get(i - 1).ok_or_else(|| {
                            format!(
                                "$<{ident}>: index [{i}] is out of range (the array has {} elements)",
                                items.len()
                            )
                        })?
                    }
                    other => {
                        return Err(format!(
                            "$<{ident}>: cannot index {} {} value",
                            article(other),
                            json_type_name(other)
                        ))
                    }
                }
            }
        };
    }

    match value {
        JsonValue::String(s) => Ok(s.clone()),
        // `serde_json::Number`'s Display is the canonical token: itoa for
        // integers, shortest-round-trip (ryu) for floats — the same rendering
        // `encode_canonical_json` embeds, because both are serde_json's.
        JsonValue::Number(n) => Ok(n.to_string()),
        JsonValue::Bool(b) => Ok(b.to_string()),
        JsonValue::Null => Err(format!(
            "$<{ident}>: the addressed value is null and cannot be substituted \
             into a command. Address a scalar element, or read the value from a \
             Lua body via cook.probes.get."
        )),
        composite @ (JsonValue::Array(_) | JsonValue::Object(_)) => {
            let hint = match composite {
                JsonValue::Array(_) => format!("$<{ident}[1]>"),
                _ => format!("$<{ident}.FIELD>"),
            };
            Err(format!(
                "$<{ident}>: the addressed value is {} {}; a composite value has \
                 no defined command-position rendering (CS-0192). Address a \
                 scalar element instead (e.g. {hint}).",
                article(composite),
                json_type_name(composite)
            ))
        }
    }
}

#[cfg(test)]
#[path = "tests/subst_tests.rs"]
mod tests;
