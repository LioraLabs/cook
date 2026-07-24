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

// CS-0101: `file:` dispatch precedes the probe-ref colon dispatch.
#[test]
fn file_prefix_resolves_to_file_ref_not_probe() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    assert_eq!(
        resolve("file:tokens.css", &ctx),
        Resolved::FileRef { pattern: "tokens.css".to_string() }
    );
    assert_eq!(
        resolve("file:templates/*.html", &ctx),
        Resolved::FileRef { pattern: "templates/*.html".to_string() }
    );
}

#[test]
fn file_ref_absolute_path_is_error() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    assert!(matches!(
        resolve("file:/etc/passwd", &ctx),
        Resolved::Error(ResolveError::FileRefBadPath { .. })
    ));
}

#[test]
fn file_ref_parent_escape_is_error() {
    let r = empty();
    let ctx = ctx_oneshot_none(&r);
    assert!(matches!(
        resolve("file:../secret.css", &ctx),
        Resolved::Error(ResolveError::FileRefBadPath { .. })
    ));
}

#[test]
fn non_file_colon_ident_still_probe_ref() {
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
        Resolved::ProbeRef { key, access } => {
            assert_eq!(key, "cc:zlib");
            assert_eq!(access, r#"cook.probes.get("cc:zlib")"#);
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
            assert_eq!(access, r#"cook.probes.get("cc:zlib").cflags"#);
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
            assert_eq!(access, r#"cook.probes.get("cc:zlib").libs[2]"#);
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
