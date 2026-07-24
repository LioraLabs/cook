use super::*;

fn setup() -> Lua {
    let lua = Lua::new();
    register_path_api(&lua).unwrap();
    lua
}

#[test]
fn stem_strips_extension() {
    let lua = setup();
    let s: String = lua.load(r#"return path.stem("src/foo.c")"#).eval().unwrap();
    assert_eq!(s, "foo");
}

#[test]
fn ext_includes_leading_dot() {
    let lua = setup();
    let s: String = lua.load(r#"return path.ext("src/foo.c")"#).eval().unwrap();
    assert_eq!(s, ".c");
}

#[test]
fn replace_ext_accepts_with_or_without_dot() {
    let lua = setup();
    let s1: String = lua
        .load(r#"return path.replace_ext("a.c", "o")"#)
        .eval()
        .unwrap();
    let s2: String = lua
        .load(r#"return path.replace_ext("a.c", ".o")"#)
        .eval()
        .unwrap();
    assert_eq!(s1, "a.o");
    assert_eq!(s2, "a.o");
}

#[test]
fn join_handles_absolute_second_arg() {
    let lua = setup();
    let s: String = lua
        .load(r#"return path.join("a/b", "/abs")"#)
        .eval()
        .unwrap();
    assert_eq!(s, "/abs");
}
