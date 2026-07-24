use super::*;

fn empty_recipes() -> BTreeSet<String> { BTreeSet::new() }

fn ctx_os_n0(r: &BTreeSet<String>) -> ResolveCtx<'_> {
    ResolveCtx { mode: IterMode::OneShot, outputs: OutputShape::None, recipes_in_scope: r }
}

#[test]
fn empty_string() {
    let r = empty_recipes();
    let ctx = ctx_os_n0(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("", &ctx, &mut env, &mut FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"\"");
}

#[test]
fn in_in_many_to_one() {
    // CS-0130: `$<all>` is gone; `$<in>` is unit-centric and in ManyToOne
    // lowers to the same `_cook_in` local, now holding the joined set
    // (table.concat), rather than a per-item loop member.
    let r = empty_recipes();
    let ctx = ResolveCtx {
        mode: IterMode::ManyToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("ar rcs $<out> $<in>", &ctx, &mut env, &mut FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"ar rcs \" .. _cook_out .. \" \" .. _cook_in");
}

#[test]
fn out_n_in_multi_output() {
    let r = empty_recipes();
    let ctx = ResolveCtx {
        mode: IterMode::ManyToOne,
        outputs: OutputShape::Multi(2),
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("cp $<out_1> $<out_2>", &ctx, &mut env, &mut FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"cp \" .. _cook_outs[1] .. \" \" .. _cook_outs[2]");
}

#[test]
fn recipe_with_accessor() {
    let mut r = BTreeSet::new();
    r.insert("lib".to_string());
    let ctx = ctx_os_n0(&r);
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<lib>", &ctx, &mut env, &mut FileRefs::new("t")).unwrap();
    assert_eq!(result, "cook.dep_output(\"lib\")");
}
