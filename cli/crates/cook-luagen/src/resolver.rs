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
    /// CS-0240 §{xref.resolution} step 3: the probe keys a colon-free sigil may
    /// name. Derived from the Cookfile being lowered — its native `probe`,
    /// `files` and `tools` declarations — since those are the keys an author
    /// can spell without a module prefix. Colon-carrying keys need no
    /// membership check and so are absent from it.
    pub probe_keys_in_scope: &'a BTreeSet<String>,
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
    /// COOK-63 §9.3: `$<in>` — the whole current data member.
    Item,
    /// COOK-63 §9.3: `$<in.FIELD>` — record field `FIELD` of the member.
    ItemField(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolved {
    Builtin(BuiltinKind),
    Recipe {
        name: String,
        accessor: Option<String>,
    },
    /// COOK-96 / COOK-221 / CS-0137: `$<recipe[in]>` — the recipe's terminal
    /// output for the CURRENT iteration member. Resolved per consumer member
    /// inside a fan-out body.
    RecipeMember {
        name: String,
    },
    EnvRuntime(String),
    /// CS-0074: a probe-value reference — `$<key>`, `$<key.field>`, or `$<key.field[i]>`.
    /// `key` is the probe key (everything before the first `.` or `[`).
    /// CS-0195 removed the pre-built `access` Lua expression: every emission
    /// site now renders through one substitution helper keyed by the IDENT,
    /// so nothing needs a ready-made `cook.probes.get(...)` chain.
    ProbeRef {
        key: String,
    },
    Error(ResolveError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ResolveError {
    #[error("placeholder $<{ident}>: '{builtin}' is not valid in {mode:?} mode")]
    BuiltinWrongMode {
        ident: String,
        builtin: String,
        mode: IterMode,
    },
    #[error("placeholder $<{ident}>: '{builtin}' requires {required} declared output(s); step declares {actual}")]
    BuiltinWrongOutputCount {
        ident: String,
        builtin: String,
        required: String,
        actual: usize,
    },
    #[error("placeholder $<{ident}>: malformed out_N (N must be ≥ 1)")]
    MalformedOutIndex { ident: String },
    #[error("placeholder $<{ident}>: a recipe-member ref `$<recipe[in]>` is only valid inside a bare-`gather` fan-out body")]
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
    /// or, when it must be a cache determinant, through a named shell probe.
    #[error(
        "placeholder $<env.{key}>: the `env.` prefix is retired — a declared \
         variable is `$<{key}>` (or `$<var.{key}>` to disambiguate from a \
         recipe of the same name). For an ambient process variable use `${key}` \
         in the step body, or declare and seal `probe host:{key}` with \
         `lines {{ echo \"${key}\" }}` to make it a determinant."
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
         Declare `files NAME` with `\"{path}\"`, then seal `NAME` on the unit \
         that reads it; this makes the file content a \
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

/// COOK-89 §9.3: recognise the data-member binding sigils `$<in>` and
/// `$<in.FIELD>`. Returns the matching [`BuiltinKind`], or `None` for any
/// other ident.
///
/// Deliberately *not* wired into [`resolve`]: `in` is the member binding only
/// inside a data-driven (bare-`gather`) recipe body, so only
/// the member-fanout codegen path (`template::expand_member_fanout_template`) consults it.
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
///  - in a bare-`gather` fan-out recipe, `X` is a member field with
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

/// The `NAME.ACCESSOR` reading of an IDENT: the name being referenced, and the
/// path accessor applied to its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessorRef<'a> {
    /// The referenced name — a recipe, or a qualified `alias.recipe`.
    pub name: &'a str,
    /// The path accessor, always a member of [`ACCESSORS`].
    pub accessor: &'a str,
}

/// Read `ident` as `NAME.ACCESSOR`, per Standard §{xref.dotted-names}: split at
/// the RIGHTMOST `.`, admit the suffix only if it is a path accessor, and admit
/// the prefix only if it names something in scope.
///
/// The third of this module's classification predicates, beside
/// [`is_own_input_ref`] and [`is_output_ref`], and here for the same reason
/// they are: whether `$<lib.stem>` is a cross-recipe accessor reference decides,
/// at different call sites, what the substitution emits, which recipe drives a
/// step's iteration, whether the placeholder is diagnosed, and — through
/// `cook-plan`'s inferred-deps pass — whether the build closure contains the
/// producer at all. Six sites across `resolver`, `template` and `recipe` used to
/// spell the rule inline (`rfind('.')`, [`ACCESSORS`], scope lookup), which is
/// six chances for one of them to answer differently from the rest (COOK-415).
///
/// It stays in this crate rather than descending to `cook-contracts`: every
/// consumer is in `cook-luagen`, and a law with one crate's worth of consumers
/// does not meet that crate's admission bar. [`ACCESSORS`] itself is different
/// and does live there — the closed set is language surface (§{xref.path-accessors}),
/// and `cook-lang` reserves recipe segments against it.
///
/// The prefix is checked against a caller-supplied name set rather than parsed,
/// because "in scope" is the caller's knowledge: the register-phase recipe set
/// of §{xref.resolution} step 2 for a resolver, the §7.3 import union for
/// dependency extraction. The split is the law; the scope is the input.
///
/// Note what this deliberately does NOT decide: whether a builtin shape
/// (`$<in.stem>`, `$<out_2.dir>`) takes precedence over the accessor reading.
/// That ordering belongs to §{xref.resolution}'s numbered steps and stays with
/// each caller — see [`recipe_ref`] and `template::output_pattern_ident_to_lua`,
/// which order it differently and are documented as doing so.
pub fn accessor_ref<'a>(
    ident: &'a str,
    names_in_scope: &BTreeSet<String>,
) -> Option<AccessorRef<'a>> {
    let (name, accessor) = ident.rsplit_once('.')?;
    if ACCESSORS.contains(&accessor) && names_in_scope.contains(name) {
        Some(AccessorRef { name, accessor })
    } else {
        None
    }
}

/// A name reference an IDENT carries: the recipe it names, and the path
/// accessor applied to that recipe's output, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeRef {
    pub name: String,
    pub accessor: Option<String>,
}

/// The recipe an IDENT names, if any — §{xref.name-references} read off
/// §{xref.resolution}.
///
/// This is the entry point for §{xref.dep-implications}: the dependency edges
/// the engine executes are derived from the same resolution that decides what
/// the placeholder substitutes to, not from a second classification standing
/// beside it (CS-0210). `dep_ref::parse_dep_token` used to be that second
/// classification — its own builtin table, its own `env.`/`in.`/`out.`/`out_N`
/// prefix tests — and where the two disagreed the result was not a bad
/// diagnostic but a missing DAG edge, which reads a recipe's output with no
/// guarantee it was produced first.
///
/// **Context-free by construction.** [`resolve`] takes an iteration mode and an
/// output shape; this function supplies fixed ones, and that is sound rather
/// than convenient: [`match_builtin`] never consults the context, and
/// [`validate_builtin`] can only narrow an already-matched builtin into an
/// error. No context turns a builtin into a recipe reference or the reverse, so
/// no caller has to know the step it is analysing. `resolver_tests` pins it.
/// **Probe-free by construction (CS-0240).** It supplies an empty probe keyset,
/// which is sound rather than convenient. Adding a probe key can only turn an
/// answer of `None` into a different `None` — a probe is not a recipe — and it
/// can never turn a `Some` into a `None`, because App. A.2's one-name-one-kind
/// rule rejects at parse time any Cookfile where a name is both a probe key and
/// a recipe. So the edge set §{xref.dep-implications} derives from this
/// function is identical with or without the keyset, and CS-0210's requirement
/// that an analysis agree with §{xref.resolution} about what a token names is
/// met without threading a set no caller has.
pub fn recipe_ref(ident: &str, recipes_in_scope: &BTreeSet<String>) -> Option<RecipeRef> {
    let no_probes = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope,
        probe_keys_in_scope: &no_probes,
    };
    match resolve(ident, &ctx) {
        Resolved::Recipe { name, accessor } => Some(RecipeRef { name, accessor }),
        // CS-0137: `$<recipe[in]>` reads one member of the producer's output,
        // and the edge it implies is still recipe-level — the producer builds
        // first. §{xref.dep-implications} permits a finer per-unit refinement;
        // it does not permit no edge.
        Resolved::RecipeMember { name } => Some(RecipeRef {
            name,
            accessor: None,
        }),
        Resolved::Builtin(_)
        | Resolved::EnvRuntime(_)
        | Resolved::ProbeRef { .. }
        | Resolved::Error(_) => None,
    }
}

pub fn resolve(ident: &str, ctx: &ResolveCtx<'_>) -> Resolved {
    // CS-0187: the retired `file:` prefix, refused ahead of the probe
    // dispatch — the position the removed namespace occupied — so the
    // diagnostic names the retirement rather than an undeclared probe key.
    if let Some(path) = ident.strip_prefix("file:") {
        return Resolved::Error(ResolveError::RetiredFilePrefix {
            path: path.to_string(),
        });
    }

    // CS-0172 / §{xref.var-namespace}: the reserved `var.` prefix, hoisted
    // ahead of every lookup step (CS-0240). §10.7 requires `$<var.X>` to reach
    // the declared variable "regardless of whether a recipe of the same name
    // exists in scope", and the naming rules forbid `var` as the first segment
    // of a recipe name, so no earlier step could ever have claimed it —
    // hoisting is behaviour-preserving for what was already here. It is NOT
    // behaviour-preserving for the probe-name step added below, which is why it
    // had to move: a probe keyed `var` would otherwise read `$<var.X>` as field
    // `X` of that probe, and the explicit spelling would stop being explicit.
    //
    // The retired `env.` prefix deliberately did NOT move with it. `env` is
    // reserved only as a placeholder segment, not as an import alias, so a
    // qualified `$<env.foo>` naming an imported recipe is legal and must stay
    // an edge (`dep_ref_tests::cs_0210_a_qualified_name_under_a_builtin_looking_alias_is_a_dep`);
    // its refusal therefore stays below, after the recipe steps.
    if let Some(key) = ident.strip_prefix("var.") {
        return Resolved::EnvRuntime(key.to_string());
    }

    // A colon-qualified recipe name is unambiguous when it is in scope. It
    // must win before punctuation-based probe fallback so the normal recipe
    // lowering records both the dependency edge and output-content input fold.
    if ctx.recipes_in_scope.contains(ident) {
        return Resolved::Recipe {
            name: ident.to_string(),
            accessor: None,
        };
    }

    // CS-0074, amended by CS-0240: probe-value reference. A colon-carrying
    // IDENT is one on sight unless the exact identifier named the recipe above;
    // a colon-free one is one when it names a declared probe key. The colon-free
    // form is resolved BELOW, at §{xref.resolution} step 3, so a builtin shape
    // and a recipe name keep winning over a same-named probe; the parse-time
    // one-name-one-kind rule (App. A.2) makes the recipe case unreachable, and
    // the builtin case is the closed set every position shares.
    //
    // The grammar lives in `sigil` so cook-register's `cook.add_unit` capture
    // reads the same walker (COOK-357).
    if let Some(r) = crate::sigil::probe_ref(ident, cook_contracts::sigil::colon_keys_only) {
        return Resolved::ProbeRef {
            key: r.key().to_string(),
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
                "in" if ctx.recipes_in_scope.contains(&name) => Resolved::RecipeMember { name },
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
    if let Some(r) = accessor_ref(ident, ctx.recipes_in_scope) {
        return Resolved::Recipe {
            name: r.name.to_string(),
            accessor: Some(r.accessor.to_string()),
        };
    }
    // CS-0172: the retired `env.` prefix. Kept ahead of the probe step and the
    // variable fallthrough, exactly where it was: `env` is reserved as a
    // placeholder segment, so its refusal outranks a probe that happens to be
    // keyed `env`.
    if let Some(key) = ident.strip_prefix("env.") {
        return Resolved::Error(ResolveError::RetiredEnvPrefix {
            key: key.to_string(),
        });
    }
    // §{xref.resolution} step 3 (CS-0240): a declared probe key, by NAME. The
    // colon-free half of the probe rule — the half CS-0074 could not express,
    // because it read the punctuation rather than the name, leaving `probe
    // keyed_obs` declaring a probe that `$<keyed_obs>` fell past into the
    // variable step below and reported as undeclared.
    if let Some(r) = crate::sigil::probe_ref(ident, |k| ctx.probe_keys_in_scope.contains(k)) {
        return Resolved::ProbeRef {
            key: r.key().to_string(),
        };
    }
    // Otherwise a declared variable — §{xref.resolution} step 4, and the
    // fallthrough that step 5's hard error is raised from at register time.
    // The `var.` and `env.` prefixes are handled at the top of this function.
    Resolved::EnvRuntime(ident.to_string())
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
        // [`match_member_sigil`] in the member-fanout codegen path, not by `resolve` /
        // `match_builtin`. The arm exists only for exhaustiveness.
        Item | ItemField(_) => {}
    }
    Resolved::Builtin(b)
}

#[cfg(test)]
#[path = "tests/resolver_tests.rs"]
mod tests;
