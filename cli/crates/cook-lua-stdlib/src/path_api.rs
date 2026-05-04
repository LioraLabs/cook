//! `path.*` — pure string path manipulation (§6.6).
//!
//! No I/O, no working-directory dependency. Phase: Both.

use mlua::prelude::*;

pub fn register_path_api(lua: &Lua) -> LuaResult<()> {
    let path_table = lua.create_table()?;

    path_table.set(
        "stem",
        lua.create_function(|_, p: String| {
            Ok(std::path::Path::new(&p)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default())
        })?,
    )?;

    path_table.set(
        "name",
        lua.create_function(|_, p: String| {
            Ok(std::path::Path::new(&p)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default())
        })?,
    )?;

    path_table.set(
        "ext",
        lua.create_function(|_, p: String| {
            Ok(std::path::Path::new(&p)
                .extension()
                .map(|s| format!(".{}", s.to_string_lossy()))
                .unwrap_or_default())
        })?,
    )?;

    path_table.set(
        "dir",
        lua.create_function(|_, p: String| {
            Ok(std::path::Path::new(&p)
                .parent()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default())
        })?,
    )?;

    path_table.set(
        "replace_ext",
        lua.create_function(|_, (p, new_ext): (String, String)| {
            let ext = new_ext.strip_prefix('.').unwrap_or(&new_ext);
            Ok(std::path::PathBuf::from(&p)
                .with_extension(ext)
                .to_string_lossy()
                .to_string())
        })?,
    )?;

    path_table.set(
        "join",
        lua.create_function(|_, (a, b): (String, String)| {
            Ok(std::path::PathBuf::from(&a)
                .join(&b)
                .to_string_lossy()
                .to_string())
        })?,
    )?;

    lua.globals().set("path", path_table)?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
