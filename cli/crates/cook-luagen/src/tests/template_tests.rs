use super::*;

// ─── ConsultedEnv tests ───────────────────────────────────────────────────

#[test]
fn consulted_env_to_lua_table_empty() {
    let c = ConsultedEnv::new();
    assert_eq!(c.to_lua_table(), "{}");
}

#[test]
fn consulted_env_to_lua_table_sorted() {
    let mut c = ConsultedEnv::new();
    c.record("Z");
    c.record("A");
    c.record("M");
    assert_eq!(c.to_lua_table(), "{\"A\", \"M\", \"Z\"}");
}

#[test]
fn consulted_env_record_dedups() {
    let mut c = ConsultedEnv::new();
    c.record("CFLAGS");
    c.record("CFLAGS");
    c.record("CFLAGS");
    assert_eq!(c.keys.len(), 1);
}

#[test]
fn consulted_env_to_lua_table_escapes_quotes() {
    let mut c = ConsultedEnv::new();
    c.record("KEY\"WITH\"QUOTES");
    assert!(c.to_lua_table().contains("\\\""));
}

// ─── expand_sigil_template tests ─────────────────────────────────────────

fn empty_recipes() -> BTreeSet<String> {
    BTreeSet::new()
}

fn ctx_oneone_single(recipes: &BTreeSet<String>) -> ResolveCtx<'_> {
    ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: recipes,
    }
}

fn ctx_oneshot_none(recipes: &BTreeSet<String>) -> ResolveCtx<'_> {
    ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: recipes,
    }
}

// ─── quote_context tests (CS-0128) ───────────────────────────────────────

#[test]
fn quote_context_bare() {
    assert_eq!(quote_context("echo "), QCtx::Bare);
    assert_eq!(quote_context(""), QCtx::Bare);
    // A closed double-quoted region returns to bare.
    assert_eq!(quote_context("echo \"hi\" "), QCtx::Bare);
}

#[test]
fn quote_context_double() {
    assert_eq!(quote_context("echo \"hi "), QCtx::Double);
    // A single quote inside a double-quoted region is literal, not an open.
    assert_eq!(quote_context("echo \"it's "), QCtx::Double);
}

#[test]
fn quote_context_single() {
    assert_eq!(quote_context("echo 'hi "), QCtx::Single);
    // A double quote inside a single-quoted region is literal.
    assert_eq!(quote_context("echo 'say \"hi "), QCtx::Single);
    // Backslash is inert inside single quotes (POSIX).
    assert_eq!(quote_context("echo 'a\\"), QCtx::Single);
}

#[test]
fn quote_context_backslash_escape_outside_quotes() {
    // An escaped double-quote does not open a double-quoted region.
    assert_eq!(quote_context("echo \\\" "), QCtx::Bare);
}

#[test]
fn no_placeholders_returns_quoted_literal() {
    let r = empty_recipes();
    let ctx = ctx_oneone_single(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("echo hello", &ctx, &mut env).unwrap();
    assert_eq!(result, "\"echo hello\"");
    assert!(env.keys.is_empty());
}

#[test]
fn shell_brace_idioms_survive_verbatim() {
    // {a,b,c}, ${HOME:-x}, awk '{print $1}' — none of these are $<...> so they pass through.
    let r = empty_recipes();
    let ctx = ctx_oneshot_none(&r);
    let mut env = ConsultedEnv::new();

    let result = expand_sigil_template("{a,b,c}", &ctx, &mut env).unwrap();
    assert_eq!(result, "\"{a,b,c}\"");

    let result2 = expand_sigil_template("${HOME:-x}", &ctx, &mut env).unwrap();
    assert_eq!(result2, "\"${HOME:-x}\"");

    let result3 = expand_sigil_template("awk '{print $1}'", &ctx, &mut env).unwrap();
    assert_eq!(result3, "\"awk '{print $1}'\"");

    assert!(env.keys.is_empty(), "no env keys should be recorded for shell braces");
}

#[test]
fn in_lowers_to_cook_in_in_one_to_one() {
    let r = empty_recipes();
    let ctx = ctx_oneone_single(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<in>", &ctx, &mut env).unwrap();
    assert_eq!(result, "_cook_in");
}

#[test]
fn out_lowers_to_cook_out_in_single_output() {
    let r = empty_recipes();
    let ctx = ctx_oneone_single(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<out>", &ctx, &mut env).unwrap();
    assert_eq!(result, "_cook_out");
}

#[test]
fn recipe_lowers_to_dep_output() {
    let mut r = BTreeSet::new();
    r.insert("libmath".to_string());
    let ctx = ctx_oneshot_none(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("gcc $<libmath>", &ctx, &mut env).unwrap();
    assert_eq!(result, "\"gcc \" .. cook.dep_output(\"libmath\")");
}

#[test]
fn env_var_lowers_to_require_var() {
    let r = empty_recipes();
    let ctx = ctx_oneshot_none(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<HOME>", &ctx, &mut env).unwrap();
    assert_eq!(result, "cook.require_var(\"HOME\")");
    assert!(env.keys.contains("HOME"), "HOME should be recorded");
}

#[test]
fn var_prefix_strips_to_require_var() {
    let r = empty_recipes();
    let ctx = ctx_oneshot_none(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<var.HOME>", &ctx, &mut env).unwrap();
    assert_eq!(result, "cook.require_var(\"HOME\")");
    assert!(env.keys.contains("HOME"));
}

#[test]
fn builtin_in_wrong_mode_returns_err() {
    // CS-0130: `$<in>` is unit-centric — it is legal in OneToOne (loop
    // member) and ManyToOne (joined set); only OneShot (no iteration
    // source) rejects the bare form.
    let r = empty_recipes();
    let os_ctx = ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::Single,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<in>", &os_ctx, &mut env);
    assert!(result.is_err(), "expected error for $<in> in one-shot mode");

    // $<in.ACCESSOR> requires the input set be singular — still rejected
    // outside OneToOne (both ManyToOne and OneShot).
    let m2o_ctx = ResolveCtx {
        mode: IterMode::ManyToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<in.stem>", &m2o_ctx, &mut env);
    assert!(result.is_err(), "expected error for $<in.stem> in many-to-one mode");

    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<in.stem>", &os_ctx, &mut env);
    assert!(result.is_err(), "expected error for $<in.stem> in one-shot mode");
}

#[test]
fn mixed_template_with_literal_and_placeholders() {
    let r = empty_recipes();
    let ctx = ctx_oneone_single(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("gcc -c $<in> -o $<out>", &ctx, &mut env).unwrap();
    assert_eq!(result, "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out");
}

#[test]
fn in_stem_expands_to_path_stem() {
    let r = empty_recipes();
    let ctx = ctx_oneone_single(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("build/$<in.stem>.o", &ctx, &mut env).unwrap();
    assert_eq!(result, "\"build/\" .. path.stem(_cook_in) .. \".o\"");
}

// COOK-63 §8.3: data-member builtins lower to `item` accesses.
#[test]
fn item_builtins_lower_to_member_access() {
    assert_eq!(builtin_to_lua(BuiltinKind::Item), "cook.member_to_string(item)");
    assert_eq!(
        builtin_to_lua(BuiltinKind::ItemField("host".into())),
        "cook.member_to_string(item[\"host\"])"
    );
    assert_eq!(
        builtin_to_lua(BuiltinKind::ItemField("user-id".into())),
        "cook.member_to_string(item[\"user-id\"])"
    );
}

// COOK-96: $<recipe[in]> inside a fan-out body lowers to cook.dep_output_member.
#[test]
fn recipe_member_lowers_to_dep_output_member() {
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        let ctx = cook_step_ctx(IterMode::OneShot, OutputShape::Single, &recipes);
        let mut env = ConsultedEnv::new();
        let (lua, _) = expand_for_each_template(
            "bin/mux --video $<render[in]>",
        &ctx,
        &mut env,
        ProbeLowering::CacheGet,
    )
    .unwrap();
    assert_eq!(
        lua,
        "\"bin/mux --video \" .. cook.dep_output_member(\"render\", cook.member_to_string(item))"
    );
}

// COOK-96: $<recipe[in]> in a plain (non-fan-out) command body must error.
#[test]
fn recipe_member_in_plain_command_is_error() {
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        let ctx = cook_step_ctx(IterMode::OneToOne, OutputShape::Single, &recipes);
        let mut env = ConsultedEnv::new();
        let res = expand_command_template("bin/x $<render[in]>", &ctx, &mut env);
    assert!(
        matches!(res, Err(ResolveError::RecipeMemberOutsideFanout { .. })),
        "expected RecipeMemberOutsideFanout, got: {res:?}"
    );
}

// COOK-221 / CS-0137: the pre-v1.0 spelling `$<render[]>` must error with
// a did-you-mean even inside a fan-out body.
#[test]
fn recipe_member_empty_index_errors_in_fanout_body() {
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        let ctx = cook_step_ctx(IterMode::OneShot, OutputShape::Single, &recipes);
        let mut env = ConsultedEnv::new();
        let res = expand_for_each_template(
            "bin/mux --video $<render[]>",
        &ctx,
        &mut env,
        ProbeLowering::CacheGet,
    );
    assert!(
        matches!(res, Err(ResolveError::RecipeMemberEmptyIndex { .. })),
        "expected RecipeMemberEmptyIndex, got: {res:?}"
    );
}

// COOK-221 / CS-0137: any bracket content other than the literal `in`
// is rejected (member-field joins are not part of v1.0).
#[test]
fn recipe_member_bad_index_errors_in_fanout_body() {
    let mut recipes = BTreeSet::new();
    recipes.insert("render".to_string());
        let ctx = cook_step_ctx(IterMode::OneShot, OutputShape::Single, &recipes);
        let mut env = ConsultedEnv::new();
        let res = expand_for_each_template(
            "bin/mux --video $<render[key]>",
        &ctx,
        &mut env,
        ProbeLowering::CacheGet,
    );
    assert!(
        matches!(res, Err(ResolveError::RecipeMemberBadIndex { .. })),
        "expected RecipeMemberBadIndex, got: {res:?}"
    );
}

// COOK-221 / CS-0137, retyped by COOK-357: in an OUTPUT PATTERN the
// bracket-index diagnostics are typed errors, never a fall-back to
// cook.require_var and no longer a SIGIL_ERROR string literal.
#[test]
fn recipe_member_bracket_errors_are_typed_in_output_patterns() {
    let names = BTreeSet::new();
    let mut env = ConsultedEnv::new();

    let res = expand_output_pattern("build/$<render[]>.o", &names, &mut env);
    assert!(
        matches!(res, Err(ResolveError::RecipeMemberEmptyIndex { .. })),
        "expected the did-you-mean error, got: {res:?}"
    );

    let res = expand_output_pattern("build/$<render[key]>.o", &names, &mut env);
    assert!(
        matches!(res, Err(ResolveError::RecipeMemberBadIndex { .. })),
        "expected the reserved-index error, got: {res:?}"
    );

    assert!(env.keys.is_empty(), "no env keys must be recorded for bracket errors");
}
