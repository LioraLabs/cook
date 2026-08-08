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

// ---------------------------------------------------------------------------
// CS-0206: the path form composes the SAME binding through the SAME door
// ---------------------------------------------------------------------------
//
// The two `use` forms reach one binding by different derivations — a name IS
// its alias after the §12.1 rewrite, a path's is explicit or comes from its
// basename — so a codegen that gets one right can get the other wrong. And the
// failure would be invisible where it matters most: §24.2 requires the path
// form to bind through `cook.load_module` precisely because CS-0204 observes
// that door, so a path-loaded module bound any other way would be run without
// being keyed, on a key that addresses a store shared between machines.

/// The prelude for a path-form `use`, glued onto a body that opens with the
/// alias. The TARGET is the normalised path, not the alias.
const PATH_GLUED: &str = r#"local helpers = cook.load_module("lua/helpers.lua"); helpers."#;

fn assert_path_bound(src: &str, what: &str) {
    let out = lua(src);
    assert!(
        out.contains(PATH_GLUED),
        "{what}: a path-form `use` never reached the body (§24.2, CS-0206).\n\
         Generated Lua:\n{out}"
    );
}

#[test]
fn cook_step_lua_body_binds_a_path_form_alias() {
    assert_path_bound(
        "use ./lua/helpers.lua\n\nrecipe a\n    cook \"out.txt\" >{ helpers.say() }\n",
        "cook one-shot",
    );
}

#[test]
fn test_step_lua_body_binds_a_path_form_alias() {
    assert_path_bound(
        "use ./lua/helpers.lua\n\nrecipe a\n    test >{ helpers.say() }\n",
        "test one-shot",
    );
}

#[test]
fn probe_produce_body_binds_a_path_form_alias() {
    assert_path_bound(
        "use ./lua/helpers.lua\n\nprobe p\n    >{ helpers.say() }\n",
        "probe produce",
    );
}

#[test]
fn chore_lua_body_binds_a_path_form_alias() {
    assert_path_bound(
        "use ./lua/helpers.lua\n\nchore c\n    > helpers.say()\n",
        "chore",
    );
}

#[test]
fn the_register_chunk_binds_a_path_form_alias_too() {
    let out = lua("use ./lua/helpers.lua\n\nrecipe a\n    cook \"o.txt\" { touch $<out> }\n");
    assert!(
        out.contains("local helpers = cook.load_module(\"lua/helpers.lua\")\n"),
        "register chunk missing the path-form binding:\n{out}"
    );
}

#[test]
fn an_explicit_alias_is_emitted_not_re_derived_from_the_basename() {
    let out = lua("use fmt ./lua/code-formatting.lua\n\nrecipe a\n    test >{ fmt.run() }\n");
    assert!(
        out.contains(r#"local fmt = cook.load_module("lua/code-formatting.lua"); fmt."#),
        "explicit alias must reach the body verbatim:\n{out}"
    );
    // The basename would have derived `code_formatting`. If the emitter
    // re-derived instead of reading the declaration, this would appear.
    assert!(
        !out.contains("code_formatting"),
        "emitter re-derived the alias instead of using the declared one:\n{out}"
    );
}

#[test]
fn a_hyphenated_basename_binds_the_underscore_alias_and_the_unrewritten_path() {
    // COOK-436: §12.1's hyphen-to-underscore rewrite had no reachable input
    // until CS-0206 — CS-0035 rejects a hyphen in a `use` NAME. A basename
    // passes through no name production, so it can carry one. Both halves at
    // once: the alias loses the hyphen, the path on disk does not.
    let out = lua("use ./lua/my-helpers.lua\n\nrecipe a\n    test >{ my_helpers.say() }\n");
    assert!(
        out.contains(r#"local my_helpers = cook.load_module("lua/my-helpers.lua"); my_helpers."#),
        "hyphenated basename did not derive its underscore alias:\n{out}"
    );
}

#[test]
fn a_path_form_body_that_never_names_the_alias_gets_no_binding() {
    // The CS-0205 gate is about references, and it must apply to the path form
    // for the same reason: binding anyway would evaluate the module on a worker
    // VM and make its source a determinant of a unit that never touched it.
    let out = lua("use ./lua/helpers.lua\n\nrecipe a\n    test >{ print(\"hi\") }\n");
    assert!(
        !out.contains("load_module(\"lua/helpers.lua\"); "),
        "an unreferenced path-form alias must not be bound into the body:\n{out}"
    );
    assert!(out.contains("local helpers = cook.load_module(\"lua/helpers.lua\")\n"));
}
