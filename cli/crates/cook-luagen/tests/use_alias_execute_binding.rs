//! CS-0205: every execute-phase Lua body that names a `use` alias carries the
//! binding into its own chunk.
//!
//! §24.2 has always said the codegen prepends `local foo =
//! cook.load_module("foo")` to execute-phase bodies. Two sites did it — the
//! imperative body-unit bundle and the chore body — and those two are exactly
//! the surfaces that stayed green. Every other body kind grew its own
//! `lua_code` emission and none of them learned about `uses`, so `use` +
//! `test >{ greet.say() }` died with `attempt to index a nil value (global
//! 'greet')`.
//!
//! These assertions are on the GENERATED LUA rather than on a parse tree,
//! because the payload is where the contract lives. The end-to-end half — the
//! recipe actually runs, and an edit to an alias-bound module rebuilds the unit
//! (CS-0204) — is `cli/e2e-fixtures/surface/38-use-alias-in-execute-bodies/`.

use std::collections::BTreeSet;

/// The prelude as it appears glued onto a body that opens with `greet.`.
const GLUED: &str = r#"local greet = cook.load_module("greet"); greet."#;

fn lua(src: &str) -> String {
    let cookfile = cook_lang::parse(src).expect("fixture must parse");
    let names: BTreeSet<String> = cookfile
        .recipes
        .iter()
        .map(|r| r.name.clone())
        .collect();
    cook_luagen::generate_checked(&cookfile, &names)
        .expect("fixture must lower")
        .0
}

fn assert_bound(src: &str, what: &str) {
    let out = lua(src);
    assert!(
        out.contains(GLUED),
        "{what}: execute-phase body never got the `use` binding (§24.2, CS-0205).\n\
         Generated Lua:\n{out}"
    );
}

#[test]
fn cook_step_one_shot_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    cook \"out.txt\" >{ greet.say() }\n",
        "cook one-shot",
    );
}

#[test]
fn cook_step_multi_output_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    cook \"a.txt\" \"b.txt\" >{ greet.say() }\n",
        "cook multi-output",
    );
}

#[test]
fn cook_step_many_to_one_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    ingredients \"src/*.js\"\n    cook \"dist/all.js\" >{ greet.say(inputs) }\n",
        "cook many-to-one",
    );
}

#[test]
fn cook_step_per_input_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    ingredients \"src/*.js\"\n    cook \"build/$<in.stem>.o\" >{ greet.say(input) }\n",
        "cook per-input",
    );
}

#[test]
fn cook_step_member_fanout_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nprobe cards\n    json { cat cards.json }\n\nrecipe a\n    ingredients cards\n    cook \"o/$<in.id>.svg\" >{ greet.say(item) }\n",
        "cook member fan-out",
    );
}

#[test]
fn test_step_one_shot_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    test >{ greet.say() }\n",
        "test one-shot",
    );
}

#[test]
fn test_step_one_to_one_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    ingredients \"t/*.lua\"\n    cook \"b/$<in.stem>\" { luac $<in> -o $<out> }\n    test >{ greet.say(input) }\n",
        "test one-to-one",
    );
}

#[test]
fn test_step_many_to_one_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nrecipe a\n    ingredients \"t/*.lua\"\n    test >{ greet.say(inputs) }\n",
        "test many-to-one",
    );
}

#[test]
fn test_step_member_fanout_lua_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nprobe cards\n    json { cat cards.json }\n\nrecipe a\n    ingredients cards\n    test >{ greet.say(item) }\n",
        "test member fan-out",
    );
}

#[test]
fn probe_lua_produce_body_binds_the_alias() {
    assert_bound(
        "use greet\n\nprobe p\n    >{ greet.say() }\n",
        "probe produce",
    );
}

#[test]
fn chore_lua_body_binds_the_alias() {
    // Already worked before CS-0205; pinned so the shared helper cannot regress
    // the one surface that was carrying the contract.
    assert_bound(
        "use greet\n\nchore c\n    > greet.say()\n",
        "chore",
    );
}

#[test]
fn a_body_that_never_names_the_alias_gets_no_binding() {
    // CS-0205: the prelude binds a Lua local, so a body that cannot name it
    // cannot observe it. Binding anyway would run the module on a worker VM
    // and make it a CS-0204 determinant of a unit that never touched it.
    let out = lua("use greet\n\nrecipe a\n    test >{ print(\"hi\") }\n");
    assert!(
        !out.contains("load_module(\"greet\"); "),
        "an unreferenced alias must not be bound into the body:\n{out}"
    );
    // The register-phase binding at the top of the chunk is unconditional and
    // stays exactly as it was.
    assert!(out.contains("local greet = cook.load_module(\"greet\")\n"));
}

#[test]
fn a_files_producer_keeps_its_sentinel_produce_verbatim() {
    // `@files-manifest` is compared by EQUALITY in cook-probe (`is_files_producer`).
    // A prelude glued onto it would route a synthesised producer to a worker VM.
    let out = lua("use greet\n\nprobe srcs\n    files { \"src/*.ts\" }\n");
    let sentinel = cook_contracts::probe_value::FILES_MANIFEST_PRODUCE;
    assert!(
        out.contains(&format!("produce = [[{sentinel}]]")),
        "files producer must keep the reserved sentinel verbatim:\n{out}"
    );
}
