use mlua::prelude::*;

pub fn register_fs_api(lua: &Lua, working_dir: &std::path::Path) -> LuaResult<()> {
    let fs = lua.create_table()?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "exists",
        lua.create_function(move |_, path: String| {
            Ok(wd.join(&path).exists())
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "size",
        lua.create_function(move |_, path: String| {
            let full = wd.join(&path);
            let meta = std::fs::metadata(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.size: {e}")))?;
            Ok(meta.len())
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "read",
        lua.create_function(move |_, path: String| {
            let full = wd.join(&path);
            let content = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.read: {e}")))?;
            Ok(content)
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "glob",
        lua.create_function(move |lua, pattern: String| {
            let full_pattern = wd.join(&pattern).to_string_lossy().to_string();
            let paths: Vec<String> = glob::glob(&full_pattern)
                .map_err(|e| mlua::Error::runtime(format!("fs.glob: {e}")))?
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let table = lua.create_table()?;
            for (i, path) in paths.iter().enumerate() {
                table.set(i + 1, path.as_str())?;
            }
            Ok(table)
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "mtime",
        lua.create_function(move |_, path: String| {
            let full = wd.join(&path);
            let meta = std::fs::metadata(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mtime: {e}")))?;
            let mtime = meta
                .modified()
                .map_err(|e| mlua::Error::runtime(format!("fs.mtime: {e}")))?;
            let duration = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| mlua::Error::runtime(format!("fs.mtime: {e}")))?;
            Ok(duration.as_secs_f64())
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "write",
        lua.create_function(move |_, (path, content): (String, String)| {
            let full = wd.join(&path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            }
            std::fs::write(&full, content)
                .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            Ok(())
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "mkdir_p",
        lua.create_function(move |_, path: String| {
            let full = wd.join(&path);
            std::fs::create_dir_all(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mkdir_p: {e}")))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("fs", fs)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;
    use tempfile::TempDir;

    fn setup(dir: &std::path::Path) -> Lua {
        let lua = Lua::new();
        register_fs_api(&lua, dir).unwrap();
        lua
    }

    #[test]
    fn test_fs_write_creates_file() {
        let dir = TempDir::new().unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.write("test.txt", "hello world")"#).exec().unwrap();
        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_fs_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "old").unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.write("test.txt", "new")"#).exec().unwrap();
        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "new");
    }

    #[test]
    fn test_fs_mkdir_p_creates_nested_dirs() {
        let dir = TempDir::new().unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.mkdir_p("a/b/c")"#).exec().unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[test]
    fn test_fs_mkdir_p_existing_is_ok() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("existing")).unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.mkdir_p("existing")"#).exec().unwrap();
        assert!(dir.path().join("existing").is_dir());
    }
}
