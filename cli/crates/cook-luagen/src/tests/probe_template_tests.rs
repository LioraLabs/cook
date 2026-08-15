use super::*;

// ─── expand_command_template: probe detection via $<...> sigils ──────────

#[test]
fn expand_command_template_plain_sigils_unchanged() {
    let r = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let (lua, keys) =
        expand_command_template("gcc -c $<in> -o $<out>", &ctx, &mut env).unwrap();
    assert_eq!(lua, "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out");
    assert!(keys.is_empty());
}

#[test]
fn expand_command_template_probe_only_keeps_literal_sigil() {
    let r = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneToOne,
        outputs: OutputShape::Single,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    // CS-0074: probe refs now use $<key:field> instead of {{key.field}}.
    let (lua, keys) =
        expand_command_template("$<cc:zlib.cflags> -c $<in>", &ctx, &mut env).unwrap();
    // COOK-187 / CS-0122: probe refs must NOT be wrapped in a deferred
    // function or lowered to a cache read at register time — the literal
    // `$<key:...>` sigil text stays in the command string for
    // cook.add_unit's register-time capture to rewrite.
    assert!(!lua.contains("function()"), "got: {}", lua);
    assert!(!lua.contains("cook.probes.get"), "got: {}", lua);
    assert!(lua.contains("$<cc:zlib.cflags>"), "got: {}", lua);
    // Sigil $<in> should still resolve normally.
    assert!(lua.contains("_cook_in"), "got: {}", lua);
    assert_eq!(keys.iter().next().map(String::as_str), Some("cc:zlib"));
}

#[test]
fn expand_command_template_probe_bare_key() {
    // $<cc:compiler> — no field path, bare key reference.
    let r = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let (lua, keys) =
        expand_command_template("$<cc:compiler> -c foo.c", &ctx, &mut env).unwrap();
    assert!(!lua.contains("function()"), "got: {}", lua);
    assert!(!lua.contains("cook.probes.get"), "got: {}", lua);
    assert!(lua.contains("$<cc:compiler>"), "got: {}", lua);
    assert!(keys.contains("cc:compiler"), "expected cc:compiler in keys; got: {:?}", keys);
}

#[test]
fn expand_command_template_probe_indexed_field() {
    // $<cc:zlib.libs[2]> — indexed array element.
    let r = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let (lua, keys) =
        expand_command_template("$<cc:zlib.libs[2]>", &ctx, &mut env).unwrap();
    assert!(!lua.contains("function()"), "got: {}", lua);
    assert!(!lua.contains("cook.probes.get"), "got: {}", lua);
    assert!(lua.contains("$<cc:zlib.libs[2]>"), "got: {}", lua);
    assert!(keys.contains("cc:zlib"));
}

#[test]
fn expand_command_template_multiple_probe_refs_collected() {
    let r = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let (lua, keys) =
        expand_command_template("$<cc:compiler.path> -c foo.c $<cc:zlib.cflags>", &ctx, &mut env).unwrap();
    assert!(!lua.contains("function()"), "got: {}", lua);
    assert!(!lua.contains("cook.probes.get"), "got: {}", lua);
    assert!(lua.contains("$<cc:compiler.path>"), "got: {}", lua);
    assert!(lua.contains("$<cc:zlib.cflags>"), "got: {}", lua);
    assert!(keys.contains("cc:compiler"), "keys: {:?}", keys);
    assert!(keys.contains("cc:zlib"), "keys: {:?}", keys);
}

#[test]
fn expand_command_template_no_probe_no_sigil_plain_literal() {
    let r = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::OneShot,
        outputs: OutputShape::None,
        recipes_in_scope: &r,
    };
    let mut env = ConsultedEnv::new();
    let (lua, keys) = expand_command_template("echo hello", &ctx, &mut env).unwrap();
    assert_eq!(lua, "\"echo hello\"");
    assert!(keys.is_empty());
}
