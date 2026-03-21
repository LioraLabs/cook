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
