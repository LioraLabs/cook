use super::*;

/// Install a Lua function that answers with the line the caller is on,
/// resolved against `target`, then run `code` under chunk name `chunk`.
fn line_seen_from(chunk: &str, target: &'static str, code: &str) -> Option<usize> {
    let lua = mlua::Lua::new();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None::<Option<usize>>));
    let sink = std::sync::Arc::clone(&seen);
    let probe = lua
        .create_function(move |lua, ()| {
            *sink.lock().unwrap() = Some(caller_line_in_source(lua, target));
            Ok(())
        })
        .unwrap();
    lua.globals().set("where", probe).unwrap();
    lua.load(code).set_name(chunk).exec().unwrap();
    let answer = seen.lock().unwrap().take();
    answer.expect("the probe was called")
}

#[test]
fn reports_the_line_of_the_call_in_the_named_chunk() {
    assert_eq!(
        line_seen_from(
            "@Cookfile",
            "@Cookfile",
            "local a = 1\nlocal b = 2\nwhere()\n"
        ),
        Some(3)
    );
}

/// A frame from a function defined elsewhere is skipped: the answer is the
/// line in the target chunk that entered it, which is what a reader can act
/// on. This is the module case — a Cookfile calls a module helper and the
/// helper shells out.
#[test]
fn skips_frames_from_other_chunks_and_reports_the_entering_line() {
    let lua = mlua::Lua::new();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None::<Option<usize>>));
    let sink = std::sync::Arc::clone(&seen);
    let probe = lua
        .create_function(move |lua, ()| {
            *sink.lock().unwrap() = Some(caller_line_in_source(lua, "@Cookfile"));
            Ok(())
        })
        .unwrap();
    lua.globals().set("where", probe).unwrap();
    lua.load("function helper() where() end")
        .set_name("@/abs/path/mod.lua")
        .exec()
        .unwrap();
    lua.load("local x = 1\nhelper()\n")
        .set_name("@Cookfile")
        .exec()
        .unwrap();

    assert_eq!(seen.lock().unwrap().take().flatten(), Some(2));
}

/// The suffix match is what lets a caller holding a bare path find a chunk
/// the loader named with Lua's `@` file prefix.
#[test]
fn matches_a_chunk_name_by_suffix() {
    assert_eq!(
        line_seen_from("@sub/Cookfile", "sub/Cookfile", "where()\n"),
        Some(1)
    );
}

/// No frame from the target chunk means no line — the caller degrades to a
/// location-free diagnostic rather than reporting somebody else's line.
#[test]
fn reports_nothing_when_the_target_chunk_is_not_on_the_stack() {
    assert_eq!(
        line_seen_from("@probe:cc:zlib", "@Cookfile", "where()\n"),
        None
    );
}
