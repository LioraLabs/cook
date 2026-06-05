//! Closed-set placeholder resolver per CS-0033 §3.2.
//!
//! Given an IDENT from the sigil scanner plus the current step's context,
//! decide whether the placeholder resolves to a builtin, an in-scope recipe,
//! or an env-var runtime lookup. Codegen-time errors are limited to builtin
//! mode/count violations; the env-declared check is deferred to runtime via
//! `cook.require_env` (see cook-register/env_api.rs from Task 9).

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
    All,                   // {all}
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
    EnvRuntime(String),
    /// CS-0074: a probe-value reference — `$<key>`, `$<key.field>`, or `$<key.field[i]>`.
    /// `key` is the probe key (everything before the first `.` or `[`).
    /// `access` is the ready-to-emit Lua expression (e.g. `cook.cache.get("cc:zlib").cflags`).
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

/// Parse the ident of a probe-shaped sigil into `(key, lua_access_expr)`.
///
/// `ident` must contain `:`. Everything before the first `.` or `[` that
/// follows the `:` is the key; the rest is the path. Returns the Lua expression
/// that reads `cook.cache.get(key)` with the path appended.
fn parse_probe_ref(ident: &str, escape: impl Fn(&str) -> String) -> (String, String) {
    // Find the boundary between key and path. The key ends at the first `.` or `[`
    // that appears after the `:` discriminator.
    let colon_pos = ident.find(':').expect("probe ident must contain ':'");
    let after_colon = &ident[colon_pos + 1..];
    let path_start = after_colon
        .find(|c: char| c == '.' || c == '[')
        .map(|p| colon_pos + 1 + p)
        .unwrap_or(ident.len());

    let key = &ident[..path_start];
    let path_str = &ident[path_start..];

    let mut access = format!("cook.cache.get(\"{}\")", escape(key));

    // Walk the path string, building `.field` or `[N]` accesses.
    let mut chars = path_str.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '.' => {
                chars.next();
                let mut name = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc.is_alphanumeric() || nc == '_' {
                        name.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !name.is_empty() {
                    access.push('.');
                    access.push_str(&name);
                }
            }
            '[' => {
                chars.next();
                let mut idx = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc == ']' {
                        chars.next();
                        break;
                    }
                    idx.push(nc);
                    chars.next();
                }
                access.push('[');
                access.push_str(&idx);
                access.push(']');
            }
            _ => { chars.next(); }
        }
    }

    (key.to_string(), access)
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

pub fn resolve(ident: &str, ctx: &ResolveCtx<'_>) -> Resolved {
    // CS-0074: probe-value reference — IDENT contains `:`.
    // Dispatched before builtin/recipe/env so the colon discriminator is
    // unambiguous (no builtin or recipe name can contain `:`).
    if ident.contains(':') {
        let (key, access) = parse_probe_ref(ident, |s| {
            // Escape for Lua double-quoted string (minimal — just `\` and `"`).
            s.replace('\\', "\\\\").replace('"', "\\\"")
        });
        return Resolved::ProbeRef { key, access };
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
    // Otherwise env-runtime. Strip the explicit "env." prefix if present.
    let env_key = ident.strip_prefix("env.").unwrap_or(ident);
    Resolved::EnvRuntime(env_key.to_string())
}

fn match_builtin(ident: &str) -> BuiltinMatch {
    match ident {
        "in" => BuiltinMatch::Yes(BuiltinKind::In),
        "out" => BuiltinMatch::Yes(BuiltinKind::Out),
        "all" => BuiltinMatch::Yes(BuiltinKind::All),
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
        In | InAccessor(_) => {
            if ctx.mode != OneToOne {
                return Resolved::Error(ResolveError::BuiltinWrongMode {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    mode: ctx.mode,
                });
            }
        }
        All => {
            if ctx.mode != ManyToOne {
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
mod tests {
    use super::*;

    fn ctx_oneone_single<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
        ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: recipes }
    }
    fn ctx_oneshot_none<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
        ResolveCtx { mode: IterMode::OneShot, outputs: OutputShape::None, recipes_in_scope: recipes }
    }
    fn empty() -> BTreeSet<String> { BTreeSet::new() }

    #[test]
    fn member_sigil_matches_in_head() {
        assert_eq!(match_member_sigil("in"), Some(BuiltinKind::Item));
        assert_eq!(
            match_member_sigil("in.host"),
            Some(BuiltinKind::ItemField("host".to_string()))
        );
        assert_eq!(
            match_member_sigil("in.user_id"),
            Some(BuiltinKind::ItemField("user_id".to_string()))
        );
        // No longer special: the old `item` head is now an ordinary ident.
        assert_eq!(match_member_sigil("item"), None);
        assert_eq!(match_member_sigil("item.host"), None);
        // Path-accessor look-alikes are fields in member context (intentional).
        assert_eq!(
            match_member_sigil("in.stem"),
            Some(BuiltinKind::ItemField("stem".to_string()))
        );
        assert_eq!(match_member_sigil("in."), None); // empty field
        assert_eq!(match_member_sigil("ins"), None); // not the bare `in` token
    }

    // CS-0074: probe-ref dispatch tests
    #[test]
    fn probe_ref_bare_key_resolves_to_cache_get() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        match resolve("cc:zlib", &ctx) {
            Resolved::ProbeRef { key, access } => {
                assert_eq!(key, "cc:zlib");
                assert_eq!(access, r#"cook.cache.get("cc:zlib")"#);
            }
            other => panic!("expected ProbeRef, got {other:?}"),
        }
    }

    #[test]
    fn probe_ref_key_dot_field_resolves_to_field_access() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        match resolve("cc:zlib.cflags", &ctx) {
            Resolved::ProbeRef { key, access } => {
                assert_eq!(key, "cc:zlib");
                assert_eq!(access, r#"cook.cache.get("cc:zlib").cflags"#);
            }
            other => panic!("expected ProbeRef, got {other:?}"),
        }
    }

    #[test]
    fn probe_ref_key_field_index_resolves_to_indexed_access() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        match resolve("cc:zlib.libs[2]", &ctx) {
            Resolved::ProbeRef { key, access } => {
                assert_eq!(key, "cc:zlib");
                assert_eq!(access, r#"cook.cache.get("cc:zlib").libs[2]"#);
            }
            other => panic!("expected ProbeRef, got {other:?}"),
        }
    }

    #[test]
    fn probe_ref_does_not_intercept_bare_in() {
        let r = empty();
        let ctx = ctx_oneone_single(&r);
        assert_eq!(resolve("in", &ctx), Resolved::Builtin(BuiltinKind::In));
    }

    #[test]
    fn probe_ref_does_not_intercept_recipe() {
        let mut r = BTreeSet::new();
        r.insert("my_recipe".to_string());
        let ctx = ctx_oneshot_none(&r);
        assert!(matches!(resolve("my_recipe", &ctx), Resolved::Recipe { .. }));
    }

    #[test]
    fn resolves_in_to_builtin() {
        let r = empty();
        let ctx = ctx_oneone_single(&r);
        assert_eq!(resolve("in", &ctx), Resolved::Builtin(BuiltinKind::In));
    }

    #[test]
    fn resolves_in_stem_to_builtin() {
        let r = empty();
        let ctx = ctx_oneone_single(&r);
        assert_eq!(resolve("in.stem", &ctx), Resolved::Builtin(BuiltinKind::InAccessor("stem".to_string())));
    }

    #[test]
    fn resolves_recipe_in_scope() {
        let mut r = BTreeSet::new();
        r.insert("build".to_string());
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(
            resolve("build", &ctx),
            Resolved::Recipe { name: "build".to_string(), accessor: None }
        );
    }

    #[test]
    fn resolves_recipe_accessor() {
        let mut r = BTreeSet::new();
        r.insert("lib".to_string());
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(
            resolve("lib.stem", &ctx),
            Resolved::Recipe { name: "lib".to_string(), accessor: Some("stem".to_string()) }
        );
    }

    #[test]
    fn unknown_token_falls_through_to_env_runtime() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(resolve("HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
    }

    #[test]
    fn explicit_env_prefix_strips_to_env_runtime() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(resolve("env.HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
    }

    #[test]
    fn explicit_env_overrides_recipe_match() {
        let mut r = BTreeSet::new();
        r.insert("HOME".to_string());
        let ctx = ctx_oneshot_none(&r);
        // Bare HOME → recipe (recipe wins over env).
        assert!(matches!(resolve("HOME", &ctx), Resolved::Recipe { .. }));
        // env.HOME → always env, even if HOME is a recipe.
        assert_eq!(resolve("env.HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
    }

    #[test]
    fn in_in_many_to_one_is_error() {
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Single,
            recipes_in_scope: &r,
        };
        assert!(matches!(resolve("in", &ctx), Resolved::Error(ResolveError::BuiltinWrongMode { .. })));
    }

    #[test]
    fn out_in_multi_output_is_error() {
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        assert!(matches!(resolve("out", &ctx), Resolved::Error(ResolveError::BuiltinWrongOutputCount { .. })));
    }

    #[test]
    fn out_n_overflow_is_error() {
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        assert!(matches!(resolve("out_3", &ctx), Resolved::Error(ResolveError::BuiltinWrongOutputCount { .. })));
    }

    #[test]
    fn out_zero_is_lexically_valid_but_semantically_rejected() {
        // The lexer accepts `$<out_0>` (matches `out_` DIGIT+); the resolver
        // rejects N=0 as MalformedOutIndex per §xref.resolution step 1
        // (out_N MUST have N in 1..=K). The error is a hard stop — it does
        // NOT fall through to recipe-then-env lookup.
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        assert!(matches!(
            resolve("out_0", &ctx),
            Resolved::Error(ResolveError::MalformedOutIndex { .. })
        ));
    }

    #[test]
    fn out_zero_with_accessor_is_also_malformed() {
        // `$<out_0.stem>` similarly hits N=0 before the accessor is examined.
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        assert!(matches!(
            resolve("out_0.stem", &ctx),
            Resolved::Error(ResolveError::MalformedOutIndex { .. })
        ));
    }
}
