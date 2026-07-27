//! Closed-set placeholder resolver per CS-0033 §3.2.
//!
//! Given an IDENT from the sigil scanner plus the current step's context,
//! decide whether the placeholder resolves to a builtin, an in-scope recipe,
//! or an env-var runtime lookup. Codegen-time errors are limited to builtin
//! mode/count violations; the env-declared check is deferred to runtime via
//! `cook.require_var` (see cook-register/var_api.rs from Task 9).

use std::collections::BTreeSet;

use cook_contracts::ACCESSORS;

/// Iteration mode of the enclosing step (for builtin validity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IterMode {
    OneToOne,
    ManyToOne,
    OneShot,
}

/// Output declaration shape of the enclosing step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputShape {
    /// No declared outputs (plate, test, bare shell command).
    None,
    /// Exactly one declared output (single-output cook step).
    Single,
    /// N declared outputs (multi-output cook step).
    Multi(usize),
}

/// Context passed to the resolver for a single shell-text scan.
pub struct ResolveCtx<'a> {
    pub mode: IterMode,
    pub outputs: OutputShape,
    pub recipes_in_scope: &'a BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuiltinKind {
    In,                    // {in}
    InAccessor(String),    // {in.stem} etc — accessor stored
    Out,                   // {out}
    OutAccessor(String),   // {out.stem} etc
    OutIndexed(usize),     // {out_1}
    OutIndexedAccessor(usize, String), // {out_1.stem}
    /// COOK-63 §8.3: `$<in>` — the whole current `for_each` data member.
    Item,
    /// COOK-63 §8.3: `$<in.FIELD>` — record field `FIELD` of the member.
    ItemField(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolved {
    Builtin(BuiltinKind),
    Recipe { name: String, accessor: Option<String> },
    /// COOK-96 / COOK-221 / CS-0137: `$<recipe[in]>` — the recipe's terminal
    /// output for the CURRENT iteration member. Resolved per consumer member
    /// inside a fan-out body.
    RecipeMember { name: String },
    EnvRuntime(String),
    /// CS-0074: a probe-value reference — `$<key>`, `$<key.field>`, or `$<key.field[i]>`.
    /// `key` is the probe key (everything before the first `.` or `[`).
    /// `access` is the ready-to-emit Lua expression (e.g. `cook.probes.get("cc:zlib").cflags`).
    ProbeRef { key: String, access: String },
    Error(ResolveError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ResolveError {
    #[error("placeholder $<{ident}>: '{builtin}' is not valid in {mode:?} mode")]
    BuiltinWrongMode { ident: String, builtin: String, mode: IterMode },
    #[error("placeholder $<{ident}>: '{builtin}' requires {required} declared output(s); step declares {actual}")]
    BuiltinWrongOutputCount { ident: String, builtin: String, required: String, actual: usize },
    #[error("placeholder $<{ident}>: malformed out_N (N must be ≥ 1)")]
    MalformedOutIndex { ident: String },
    #[error("placeholder $<{ident}>: a recipe-member ref `$<recipe[in]>` is only valid inside an `ingredients <probe>` fan-out body")]
    RecipeMemberOutsideFanout { ident: String },
    #[error("placeholder $<{ident}>: `$<{name}[]>` was respelled `$<{name}[in]>` in v1.0")]
    RecipeMemberEmptyIndex { ident: String, name: String },
    #[error("placeholder $<{ident}>: invalid member index `[{index}]` — the only valid index is the literal `[in]`; member-field joins are not part of v1.0")]
    RecipeMemberBadIndex { ident: String, index: String },
    #[error("placeholder $<{ident}>: `[in]` names a per-member recipe output, but no recipe named '{name}' is in scope")]
    RecipeMemberUnknownRecipe { ident: String, name: String },
    /// CS-0172: `$<env.NAME>` named the process-environment namespace, which no
    /// longer backs declared variables. `$<var.NAME>` is the explicit spelling
    /// for a declared variable; an ambient process variable is read either as
    /// an ordinary shell variable (`$NAME` — a step inherits the environment)
    /// or, when it must be a cache determinant, through an `envs { }` probe.
    #[error(
        "placeholder $<env.{key}>: the `env.` prefix is retired — a declared \
         variable is `$<{key}>` (or `$<var.{key}>` to disambiguate from a \
         recipe of the same name). For an ambient process variable use `${key}` \
         in the step body, or declare an `envs {{ {key} }}` probe to make it a \
         determinant."
    )]
    RetiredEnvPrefix { key: String },
    /// CS-0187: `$<file:PATH>` is removed. Without a diagnostic the retired
    /// form does not fail cleanly — it falls through to the probe colon
    /// dispatch, so `$<file:tokens.css>` reports an undeclared probe key
    /// `file:tokens`, and a path containing `/` strict-bails to literal shell
    /// text instead. Both are worse than being told what happened, which is
    /// the same reasoning CS-0172 applied to the retired `env.` prefix.
    #[error(
        "placeholder $<file:{path}>: the `file:` prefix is retired (CS-0187). \
         Declare the file as an input instead: a `files {{ \"{path}\" }}` probe \
         sealed on the unit that reads it (`seal <probe>`) makes its content a \
         cache determinant, and the step body names the path directly."
    )]
    RetiredFilePrefix { path: String },
}

/// Three-way outcome of `match_builtin`:
/// - `Yes(b)` — ident is a well-formed builtin; mode/count validation still pending.
/// - `Malformed(e)` — ident has builtin shape but is structurally invalid (e.g. `out_0`);
///   MUST produce a load-time diagnostic, MUST NOT fall through to recipe/env lookup.
/// - `No` — ident does not look like a builtin at all; try recipe/env next.
enum BuiltinMatch {
    Yes(BuiltinKind),
    Malformed(ResolveError),
    No,
}

/// COOK-89 §8.3: recognise the data-member binding sigils `$<in>` and
/// `$<in.FIELD>`. Returns the matching [`BuiltinKind`], or `None` for any
/// other ident.
///
/// Deliberately *not* wired into [`resolve`]: `in` is the member binding only
/// inside a data-driven (for_each / `ingredients <probe>`) recipe body, so only
/// the for_each codegen path (`template::expand_for_each_template`) consults it.
/// In a glob recipe, `$<in>` keeps its file-path meaning via `match_builtin`.
pub fn match_member_sigil(ident: &str) -> Option<BuiltinKind> {
    if ident == "in" {
        return Some(BuiltinKind::Item);
    }
    match ident.strip_prefix("in.") {
        Some(field) if !field.is_empty() => Some(BuiltinKind::ItemField(field.to_string())),
        _ => None,
    }
}

/// True when `ident` references the step's own iteration source.
///
/// The one predicate for "does this sigil make the step iterate", used by
/// output-pattern classification and plate/test mode detection. Three call
/// sites used to spell it inline as `ident == "in" || ident.starts_with("in.")`
/// (COOK-357).
///
/// It deliberately spans BOTH readings of `$<in.X>`, because a pattern is
/// classified before the recipe shape is known:
///
///  - in a glob recipe, `X` is a path accessor ([`match_builtin`]);
///  - in an `ingredients <probe>` fan-out recipe, `X` is a member field with
///    an author-chosen name ([`match_member_sigil`]) — `$<in.id>` over
///    `[{"id":"intro"}]` is ordinary, and narrowing this to the accessor set
///    made every such recipe look like a literal-output gather step and be
///    rejected.
///
/// Answering `true` for a nonsense `$<in.bogus>` in a glob recipe is correct
/// here: this decides the iteration mode the author asked for, and the
/// expander then reports the ident itself.
pub fn is_own_input_ref(ident: &str) -> bool {
    matches!(
        match_builtin(ident),
        BuiltinMatch::Yes(BuiltinKind::In | BuiltinKind::InAccessor(_))
    ) || match_member_sigil(ident).is_some()
}

/// True when `ident` names a declared output: `$<out>`, `$<out.ACC>`,
/// `$<out_N>`, `$<out_N.ACC>`.
///
/// Companion to [`is_own_input_ref`], for surfaces that reject outputs
/// outright (a plate or test step declares none) rather than checking a count.
/// A malformed `out_N` counts: `$<out_0>` is an output reference that happens
/// to be invalid, and reporting it as a variable name helps nobody.
pub fn is_output_ref(ident: &str) -> bool {
    matches!(
        match_builtin(ident),
        BuiltinMatch::Yes(
            BuiltinKind::Out
                | BuiltinKind::OutAccessor(_)
                | BuiltinKind::OutIndexed(_)
                | BuiltinKind::OutIndexedAccessor(..)
        ) | BuiltinMatch::Malformed(ResolveError::MalformedOutIndex { .. })
    )
}

pub fn resolve(ident: &str, ctx: &ResolveCtx<'_>) -> Resolved {
    // CS-0187: the retired `file:` prefix, refused ahead of the probe colon
    // dispatch — the position the removed namespace occupied — so the
    // diagnostic names the retirement rather than an undeclared probe key.
    if let Some(path) = ident.strip_prefix("file:") {
        return Resolved::Error(ResolveError::RetiredFilePrefix {
            path: path.to_string(),
        });
    }

    // CS-0074: probe-value reference — IDENT contains `:`.
    // Dispatched before builtin/recipe/env so the colon discriminator is
    // unambiguous (no builtin or recipe name can contain `:`). The grammar
    // lives in `sigil` so cook-register's `cook.add_unit` capture reads the
    // same walker (COOK-357).
    if let Some(r) = crate::sigil::probe_ref(ident) {
        return Resolved::ProbeRef {
            key: r.key().to_string(),
            access: r.lua_access(),
        };
    }

    // COOK-221 / CS-0137: `$<recipe[in]>` — per-member cross-recipe output.
    // The literal index `in` names the current member binding. A trailing
    // bracket group is member-ref territory — no env fallthrough: the empty
    // index `[]` (the pre-v1.0 spelling) gets a did-you-mean, any other
    // content is rejected (member-field joins are not part of v1.0), and
    // `[in]` on an unknown name errors naming the unknown recipe.
    if ident.ends_with(']') {
        if let Some(open) = ident.find('[') {
            let name = ident[..open].to_string();
            let index = &ident[open + 1..ident.len() - 1];
            return match index {
                "in" if ctx.recipes_in_scope.contains(&name) => {
                    Resolved::RecipeMember { name }
                }
                "in" => Resolved::Error(ResolveError::RecipeMemberUnknownRecipe {
                    ident: ident.to_string(),
                    name,
                }),
                "" => Resolved::Error(ResolveError::RecipeMemberEmptyIndex {
                    ident: ident.to_string(),
                    name,
                }),
                other => Resolved::Error(ResolveError::RecipeMemberBadIndex {
                    ident: ident.to_string(),
                    index: other.to_string(),
                }),
            };
        }
    }

    // Try builtin first.
    match match_builtin(ident) {
        BuiltinMatch::Yes(b) => return validate_builtin(ident, b, ctx),
        BuiltinMatch::Malformed(e) => return Resolved::Error(e),
        BuiltinMatch::No => {}
    }
    // Try recipe (own-name or recipe.accessor).
    if ctx.recipes_in_scope.contains(ident) {
        return Resolved::Recipe { name: ident.to_string(), accessor: None };
    }
    if let Some(dot) = ident.rfind('.') {
        let prefix = &ident[..dot];
        let suffix = &ident[dot + 1..];
        if ACCESSORS.contains(&suffix) && ctx.recipes_in_scope.contains(prefix) {
            return Resolved::Recipe {
                name: prefix.to_string(),
                accessor: Some(suffix.to_string()),
            };
        }
    }
    // Otherwise a declared variable. CS-0172: `var.` is the explicit prefix
    // that disambiguates a variable from a same-named recipe; the pre-CS-0172
    // `env.` prefix is retired along with the process-env namespace it named.
    if let Some(key) = ident.strip_prefix("env.") {
        return Resolved::Error(ResolveError::RetiredEnvPrefix { key: key.to_string() });
    }
    let var_key = ident.strip_prefix("var.").unwrap_or(ident);
    Resolved::EnvRuntime(var_key.to_string())
}

fn match_builtin(ident: &str) -> BuiltinMatch {
    match ident {
        "in" => BuiltinMatch::Yes(BuiltinKind::In),
        "out" => BuiltinMatch::Yes(BuiltinKind::Out),
        _ => {
            if let Some(rest) = ident.strip_prefix("in.") {
                if ACCESSORS.contains(&rest) {
                    return BuiltinMatch::Yes(BuiltinKind::InAccessor(rest.to_string()));
                }
                BuiltinMatch::No
            } else if let Some(rest) = ident.strip_prefix("out.") {
                if ACCESSORS.contains(&rest) {
                    return BuiltinMatch::Yes(BuiltinKind::OutAccessor(rest.to_string()));
                }
                BuiltinMatch::No
            } else if let Some(rest) = ident.strip_prefix("out_") {
                let (num_str, acc) = match rest.find('.') {
                    Some(dot) => (&rest[..dot], Some(&rest[dot + 1..])),
                    None => (rest, None),
                };
                let n: usize = match num_str.parse() {
                    Ok(v) => v,
                    Err(_) => return BuiltinMatch::No,
                };
                if n == 0 {
                    // §xref.resolution step 1: out_0 is a malformed builtin shape —
                    // N MUST be ≥ 1. Return a hard error rather than falling through to
                    // recipe/env so the diagnostic names the exact problem.
                    return BuiltinMatch::Malformed(ResolveError::MalformedOutIndex {
                        ident: ident.to_string(),
                    });
                }
                match acc {
                    None => BuiltinMatch::Yes(BuiltinKind::OutIndexed(n)),
                    Some(a) if ACCESSORS.contains(&a) => {
                        BuiltinMatch::Yes(BuiltinKind::OutIndexedAccessor(n, a.to_string()))
                    }
                    _ => BuiltinMatch::No,
                }
            } else {
                BuiltinMatch::No
            }
        }
    }
}

fn validate_builtin(ident: &str, b: BuiltinKind, ctx: &ResolveCtx<'_>) -> Resolved {
    use BuiltinKind::*;
    use IterMode::*;
    use OutputShape::*;

    match &b {
        In => {
            // $<in> is the unit's input set — the loop member in one-to-one, the
            // space-joined collected set in many-to-one. Only one-shot (no
            // iteration source) rejects it.
            if ctx.mode == OneShot {
                return Resolved::Error(ResolveError::BuiltinWrongMode {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    mode: ctx.mode,
                });
            }
        }
        InAccessor(_) => {
            // An accessor requires the input set be singular — legal only in
            // one-to-one (fan-out) mode; in many-to-one the set is collected and a
            // path accessor on the joined form is meaningless.
            if ctx.mode != OneToOne {
                return Resolved::Error(ResolveError::BuiltinWrongMode {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    mode: ctx.mode,
                });
            }
        }
        Out | OutAccessor(_) => {
            if !matches!(ctx.outputs, Single) {
                let actual = match ctx.outputs {
                    None => 0,
                    Single => 1,
                    Multi(n) => n,
                };
                return Resolved::Error(ResolveError::BuiltinWrongOutputCount {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    required: "exactly 1".to_string(),
                    actual,
                });
            }
        }
        OutIndexed(n) | OutIndexedAccessor(n, _) => {
            if let Multi(declared) = ctx.outputs {
                if *n > declared {
                    return Resolved::Error(ResolveError::BuiltinWrongOutputCount {
                        ident: ident.to_string(),
                        builtin: format!("{:?}", b),
                        required: format!("≥ {}", n),
                        actual: declared,
                    });
                }
            } else {
                let actual = match ctx.outputs {
                    None => 0,
                    Single => 1,
                    Multi(n) => n,
                };
                return Resolved::Error(ResolveError::BuiltinWrongOutputCount {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    required: format!("multi-output ≥ {}", n),
                    actual,
                });
            }
        }
        // `$<in>` / `$<in.FIELD>` never arrive here: they are matched by
        // [`match_member_sigil`] in the for_each codegen path, not by `resolve` /
        // `match_builtin`. The arm exists only for exhaustiveness.
        Item | ItemField(_) => {}
    }
    Resolved::Builtin(b)
}

#[cfg(test)]
#[path = "tests/resolver_tests.rs"]
mod tests;
