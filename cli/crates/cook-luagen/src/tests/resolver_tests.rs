use super::*;

fn ctx_oneone_single<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
    ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: recipes,
    }
}
fn ctx_oneshot_none<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
    ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: recipes,
    }
}
fn empty() -> BTreeSet<String> {
    BTreeSet::new()
}

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

// CS-0101: `file:` dispatch precedes the probe-ref colon dispatch.
#[test]
fn a_colon_ident_is_a_probe_ref() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    assert!(matches!(resolve("cc:zlib.cflags", &ctx), Resolved::ProbeRef { .. }));
}

// CS-0074: probe-ref dispatch tests
#[test]
fn probe_ref_bare_key_resolves_to_cache_get() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    match resolve("cc:zlib", &ctx) {
        Resolved::ProbeRef { key } => {
            assert_eq!(key, "cc:zlib");
        }
        other => panic!("expected ProbeRef, got {other:?}"),
    }
}

#[test]
fn probe_ref_key_dot_field_resolves_to_field_access() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    match resolve("cc:zlib.cflags", &ctx) {
        Resolved::ProbeRef { key } => {
            assert_eq!(key, "cc:zlib");
            // CS-0195: no pre-built access expression; the emission sites
            // render through cook.__probe_subst keyed by the IDENT.
        }
        other => panic!("expected ProbeRef, got {other:?}"),
    }
}

#[test]
fn probe_ref_key_field_index_resolves_to_indexed_access() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    match resolve("cc:zlib.libs[2]", &ctx) {
        Resolved::ProbeRef { key } => {
            assert_eq!(key, "cc:zlib");
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
fn explicit_var_prefix_strips_to_var_runtime() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    assert_eq!(resolve("var.HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
}

#[test]
fn retired_env_prefix_is_an_error() {
    // CS-0172: `env.` named the process-environment namespace, which no longer
    // backs declared variables.
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    assert!(matches!(
        resolve("env.HOME", &ctx),
        Resolved::Error(ResolveError::RetiredEnvPrefix { .. })
    ));
}

#[test]
fn explicit_var_prefix_overrides_recipe_match() {
    let mut r = BTreeSet::new();
    r.insert("HOME".to_string());
    let ctx = ctx_oneshot_none(&r);
    // Bare HOME → recipe (recipe wins over a declared variable).
    assert!(matches!(resolve("HOME", &ctx), Resolved::Recipe { .. }));
    // var.HOME → always the variable, even if HOME is a recipe.
    assert_eq!(resolve("var.HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
}

#[test]
fn in_in_many_to_one_is_ok_accessor_is_error() {
    // CS-0130: `$<in>` is unit-centric — in ManyToOne it resolves to the
    // joined collected set (the `In` builtin), no longer an error.
    let r = empty();
    let ctx = ResolveCtx {
        mode: IterMode::ManyToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: &r,
    };
    assert_eq!(resolve("in", &ctx), Resolved::Builtin(BuiltinKind::In));
    // A path accessor on the joined form is still meaningless — rejected.
    assert!(matches!(
        resolve("in.stem", &ctx),
        Resolved::Error(ResolveError::BuiltinWrongMode { .. })
    ));
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

fn ctx_member<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
    ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::Single,
        recipes_in_scope: recipes,
    }
}

#[test]
fn recipe_bracket_in_resolves_to_recipe_member() {
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        assert_eq!(
            resolve("render[in]", &ctx_member(&recipes)),
        Resolved::RecipeMember { name: "render".to_string() }
        );
    }

    #[test]
    fn empty_bracket_index_is_respelled_error_with_did_you_mean() {
        let mut recipes = BTreeSet::new();
        recipes.insert("render".to_string());
    let ctx = ctx_member(&recipes);
    // The pre-v1.0 spelling errors whether or not the base names a recipe
    // (no env fallthrough for a trailing bracket group).
    let r = resolve("render[]", &ctx);
    match &r {
        Resolved::Error(e @ ResolveError::RecipeMemberEmptyIndex { .. }) => {
            let msg = e.to_string();
            assert!(msg.contains("`$<render[]>` was respelled `$<render[in]>` in v1.0"),
                "did-you-mean must show the concrete respelling; got: {msg}");
        }
        other => panic!("expected RecipeMemberEmptyIndex, got {other:?}"),
    }
    assert!(matches!(
        resolve("notarecipe[]", &ctx),
        Resolved::Error(ResolveError::RecipeMemberEmptyIndex { .. })
    ));
}

#[test]
fn non_in_bracket_content_is_rejected_not_v1() {
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        let ctx = ctx_member(&recipes);
        for ident in ["render[x]", "render[key]", "render[in.id]", "render[0]"] {
        match resolve(ident, &ctx) {
            Resolved::Error(e @ ResolveError::RecipeMemberBadIndex { .. }) => {
                assert!(e.to_string().contains("member-field joins are not part of v1.0"),
                    "diagnostic must note joins are not in v1.0; got: {e}");
            }
            other => panic!("{ident}: expected RecipeMemberBadIndex, got {other:?}"),
        }
    }
}

#[test]
fn bracket_in_on_unknown_recipe_is_error() {
    let recipes = empty();
    assert!(matches!(
        resolve("notarecipe[in]", &ctx_member(&recipes)),
        Resolved::Error(ResolveError::RecipeMemberUnknownRecipe { .. })
    ));
}

#[test]
fn non_trailing_bracket_group_falls_through_unchanged() {
    // Accessor chaining after the bracket group never existed for `$<R[]>`
    // and is NOT introduced for `$<R[in]>`: an ident whose bracket group is
    // not trailing keeps the pre-existing env-runtime fallthrough.
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        assert_eq!(
            resolve("render[in].stem", &ctx_member(&recipes)),
        Resolved::EnvRuntime("render[in].stem".to_string())
    );
}

// --- CS-0187: the retired `file:` prefix -----------------------------------

/// Removing the namespace without a diagnostic does not make the retired form
/// fail cleanly: `file:tokens.css` is all generic-ident characters, so it falls
/// through to the probe colon dispatch and reports an undeclared probe key
/// `file:tokens`. Naming the retirement is what CS-0172 did for `env.`, and it
/// is the difference between "that surface is gone, here is what replaced it"
/// and a diagnostic about a probe the author never wrote.
#[test]
fn the_retired_file_prefix_is_refused_by_name() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    let got = resolve("file:tokens.css", &ctx);
    assert!(
        matches!(&got, Resolved::Error(ResolveError::RetiredFilePrefix { path }) if path == "tokens.css"),
        "got {got:?}"
    );
    let rendered = match got {
        Resolved::Error(e) => e.to_string(),
        other => panic!("expected an error, got {other:?}"),
    };
    assert!(rendered.contains("CS-0187"), "the diagnostic names the entry: {rendered}");
    assert!(rendered.contains("files"), "and the replacement: {rendered}");
    assert!(rendered.contains("seal"), "and how it becomes a determinant: {rendered}");
}

/// A path shape the generic ident charset cannot hold (`/`) strict-bails to
/// literal text before resolution is ever reached, so the refusal above cannot
/// be the whole story — which is exactly why the diagnostic matters for the
/// shapes that DO reach it.
#[test]
fn a_slashed_retired_path_never_reaches_the_resolver() {
    assert!(
        crate::sigil::scan("$<file:templates/*.html>").is_empty(),
        "`/` is outside the generic ident charset, so the sequence stays literal"
    );
}

// ── CS-0210: `recipe_ref` is the one classification, and it is context-free ──

/// `recipe_ref` exists so dependency extraction can ask the resolver what a
/// token names instead of re-deriving it. That is only sound if the answer does
/// not depend on the step context the caller cannot supply — and it does not:
/// `match_builtin` never consults the context, and `validate_builtin` can only
/// narrow a builtin to an error, never widen it into a recipe reference. Pinned
/// here because the day it stops being true, the extractor silently starts
/// answering for a context it invented.
#[test]
fn cs_0210_recipe_ref_is_independent_of_mode_and_output_shape() {
    let mut names = BTreeSet::new();
    for n in ["libmath", "protos", "in.foo", "out_1"] {
        names.insert(n.to_string());
    }

    let modes = [IterMode::OneToOne, IterMode::ManyToOne, IterMode::OneShot];
    let shapes = [
        OutputShape::None,
        OutputShape::Single,
        OutputShape::Multi(1),
        OutputShape::Multi(4),
    ];

    for ident in [
        "in", "out", "in.stem", "out_2", "out_9.dir", "libmath", "libmath.stem", "in.foo",
        "out_1", "out_1.stem", "protos[in]", "CC",
    ] {
        let expected = recipe_ref(ident, &names);
        for mode in modes {
            for outputs in shapes {
                let ctx = ResolveCtx {
                    mode,
                    outputs,
                    recipes_in_scope: &names,
                };
                let via_resolve = match resolve(ident, &ctx) {
                    Resolved::Recipe { name, accessor } => Some(RecipeRef { name, accessor }),
                    Resolved::RecipeMember { name } => Some(RecipeRef {
                        name,
                        accessor: None,
                    }),
                    _ => None,
                };
                assert_eq!(
                    via_resolve, expected,
                    "$<{ident}> named a different referent in {mode:?}/{outputs:?}"
                );
            }
        }
    }
}

/// The `[in]` per-member form is a recipe-level edge: the producer must build
/// first, even though the reference reads one member of its output.
#[test]
fn cs_0210_recipe_ref_reports_the_member_form_as_a_recipe_edge() {
    let mut names = BTreeSet::new();
    names.insert("render".to_string());
    assert_eq!(
        recipe_ref("render[in]", &names),
        Some(RecipeRef { name: "render".to_string(), accessor: None })
    );
    // A bracket form on an unknown name is a diagnostic, not an edge.
    assert_eq!(recipe_ref("nope[in]", &names), None);
}

// ── The `NAME.ACCESSOR` split (§{xref.dotted-names}), one spelling ───────────

fn names(entries: &[&str]) -> BTreeSet<String> {
    entries.iter().map(|s| s.to_string()).collect()
}

#[test]
fn accessor_ref_splits_a_name_in_scope_carrying_a_path_accessor() {
    let scope = names(&["lib"]);
    assert_eq!(
        accessor_ref("lib.stem", &scope),
        Some(AccessorRef { name: "lib", accessor: "stem" })
    );
}

#[test]
fn accessor_ref_admits_only_the_closed_accessor_set() {
    let scope = names(&["lib"]);
    for accessor in ["stem", "name", "ext", "dir"] {
        let ident = format!("lib.{accessor}");
        assert_eq!(
            accessor_ref(&ident, &scope),
            Some(AccessorRef { name: "lib", accessor }),
            "{ident} is a path-accessor reference"
        );
    }
    // §{xref.path-accessors}: the set is closed. `bogus` is a field name, a
    // typo, or a dotted variable — never an accessor.
    assert_eq!(accessor_ref("lib.bogus", &scope), None);
    assert_eq!(accessor_ref("lib.stems", &scope), None);
}

#[test]
fn accessor_ref_requires_the_name_to_be_in_scope() {
    assert_eq!(accessor_ref("lib.stem", &names(&["other"])), None);
    assert_eq!(accessor_ref("lib.stem", &BTreeSet::new()), None);
}

#[test]
fn accessor_ref_splits_at_the_rightmost_dot_so_a_qualified_name_survives() {
    // §{xref.dotted-names}: `alias.recipe` is one name; the accessor is what
    // follows the LAST dot. Splitting leftmost would look up `alias` and lose
    // the cross-Cookfile reference entirely.
    let scope = names(&["alias.recipe"]);
    assert_eq!(
        accessor_ref("alias.recipe.stem", &scope),
        Some(AccessorRef { name: "alias.recipe", accessor: "stem" })
    );
}

#[test]
fn accessor_ref_rejects_an_ident_with_no_dot_or_a_trailing_dot() {
    let scope = names(&["stem", "lib"]);
    // A bare accessor is not a reference to a name — even when a recipe
    // happens to be spelled `stem`, there is nothing after a dot to read.
    assert_eq!(accessor_ref("stem", &scope), None);
    assert_eq!(accessor_ref("lib", &scope), None);
    // `$<lib.>` reaches §{xref.resolution} step 4 as an unresolved token; it is
    // not an accessor reference with an empty accessor.
    assert_eq!(accessor_ref("lib.", &scope), None);
}

#[test]
fn accessor_ref_reads_the_split_not_the_whole_token() {
    // The whole-token recipe lookup is the caller's, and §{xref.resolution}
    // runs it first; this only ever answers the split.
    assert_eq!(accessor_ref("alias.stem", &names(&["alias.stem"])), None);
    assert_eq!(
        accessor_ref("alias.stem", &names(&["alias"])),
        Some(AccessorRef { name: "alias", accessor: "stem" })
    );
}
