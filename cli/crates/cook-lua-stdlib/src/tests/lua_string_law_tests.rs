//! The Lua string-literal law, checked against Lua (COOK-440).
//!
//! `cook_contracts::lua_string` decides how arbitrary text becomes a
//! double-quoted Lua literal. Its own unit tests assert the escaping rule
//! against hand-written expectations, which proves the function agrees with
//! whoever wrote the test — not that Lua reads the result back as the text we
//! started with. Until COOK-398 there were two escapers to compare and the
//! standing agreement-test rule had an obvious target; there is one now, so
//! the second opinion has to come from somewhere else. It comes from the
//! interpreter that will actually load the generated program.
//!
//! This lives here because it is the lowest crate that has both the law
//! (`cook-contracts`) and an mlua VM to read it back; the law itself may not
//! depend on mlua, which is exactly why it cannot check itself.
//!
//! Both historical defects fail this test: `cook-luagen`'s escaper left a
//! carriage return raw (the chunk does not load), and `cook-register`'s
//! emitted `\0` where Lua's decimal escape consumes up to three digits (the
//! chunk loads and returns different bytes).

use cook_contracts::lua_string::literal;
use mlua::Lua;

/// Every value a Cook emitter might plausibly embed, plus the ones nobody
/// plans for.
fn battery() -> Vec<String> {
    let mut cases: Vec<String> = vec![
        String::new(),
        "cc -c main.c -o main.o".to_string(),
        "say \"hi\"".to_string(),
        r"C:\tmp\out".to_string(),
        "trailing backslash \\".to_string(),
        "a\nb".to_string(),
        "a\rb".to_string(),
        "crlf\r\nline".to_string(),
        "tab\there".to_string(),
        "\u{0}5".to_string(),
        "\u{1b}[0m".to_string(),
        "\u{7f}".to_string(),
        "100% \u{e9}quipe \u{1f600}".to_string(),
        "]]".to_string(),
        "--[[ not a comment ]]".to_string(),
        "\"".to_string(),
        "\\\"".to_string(),
    ];
    // Every C0 control on its own, and one string holding all of them.
    let all_controls: String = (0u32..0x20).filter_map(char::from_u32).collect();
    for c in all_controls.chars() {
        cases.push(c.to_string());
        // Followed by a digit: the position where a short numeric escape
        // silently changes which character was meant.
        cases.push(format!("{c}5"));
    }
    cases.push(all_controls);
    cases
}

#[test]
fn every_literal_loads_and_returns_the_text_it_was_built_from() {
    let lua = Lua::new();
    for case in battery() {
        let chunk = format!("return {}", literal(&case));
        let round_tripped: mlua::String = lua
            .load(&chunk)
            .eval()
            .unwrap_or_else(|e| panic!("chunk {chunk:?} for input {case:?} did not load: {e}"));
        assert_eq!(
            round_tripped.as_bytes().as_ref(),
            case.as_bytes(),
            "input {case:?} came back changed; chunk was {chunk:?}",
        );
    }
}

/// The literal is one Lua expression and nothing more: it cannot end early and
/// leave the rest of the emitted line as code. A value that tries to close its
/// own literal and call something is still just a string.
#[test]
fn a_hostile_value_cannot_escape_its_literal() {
    let lua = Lua::new();
    let hostile = r#"") ; os.exit(1) --"#;
    let chunk = format!("local x = {} ; return x", literal(hostile));
    let round_tripped: String = lua.load(&chunk).eval().expect("chunk did not load");
    assert_eq!(round_tripped, hostile);
}

/// The composed door call is a call, with its argument intact — the property
/// the three `cook.__probe_subst` emission sites depend on.
#[test]
fn a_door_call_reaches_the_door_with_the_argument_unchanged() {
    let lua = Lua::new();
    let ident = "$<probe:key\"weird\r>";
    let cook = lua.create_table().expect("table");
    cook.set(
        cook_contracts::registration::PROBE_SUBST_NAME,
        lua.create_function(|_, arg: mlua::String| Ok(arg))
            .expect("fn"),
    )
    .expect("set");
    lua.globals().set("cook", cook).expect("globals");

    let call = cook_contracts::registration::probe_subst_call(ident);
    let seen: mlua::String = lua
        .load(&format!("return {call}"))
        .eval()
        .unwrap_or_else(|e| panic!("call {call:?} did not load: {e}"));
    assert_eq!(seen.as_bytes().as_ref(), ident.as_bytes());
}
