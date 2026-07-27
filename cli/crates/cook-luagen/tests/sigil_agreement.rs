//! COOK-357: cook bodies and test bodies must ANSWER a sigil identically.
//!
//! These go through real codegen — `parse` then `generate_checked` — rather
//! than calling the resolver and comparing its output to itself. A test that
//! hand-builds the shared intermediate proves the intermediate exists; it does
//! not prove both adapters reach it. Every divergence below shipped with a
//! green suite for exactly that reason: the two lowering paths were each
//! covered in isolation and nothing compared them.
//!
//! What is asserted is the ANSWER, not the lowering. A test body binds its own
//! iteration variable and declares no outputs, which are real mechanism
//! differences; the classification of an ident and the diagnostic it produces
//! are not.

use std::collections::BTreeSet;

/// Lower a Cookfile source through the one public codegen entry point,
/// returning either the Lua or the rendered diagnostic.
fn codegen(src: &str) -> Result<String, String> {
    let cookfile = cook_lang::parse(src).map_err(|e| e.to_string())?;
    let names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
    cook_luagen::generate_checked(&cookfile, &names)
        .map(|(lua, _)| lua)
        .map_err(|e| e.to_string())
}

/// The same `$<IDENT>` in a `cook` body and in a `test` body, each in its own
/// minimal recipe. Returns the two diagnostics.
fn diagnostics_for(ident: &str) -> (String, String) {
    let cook_src = format!(
        "recipe c\n    ingredients \"*.txt\"\n    cook \"out/$<in.stem>.o\" {{ echo \"$<{ident}>\" > $<out> }}\n"
    );
    let test_src =
        format!("recipe t\n    ingredients \"*.txt\"\n    test {{ echo \"$<{ident}>\" }}\n");
    (
        codegen(&cook_src).err().unwrap_or_default(),
        codegen(&test_src).err().unwrap_or_default(),
    )
}

/// Strip the leading `line N: recipe 'X': ` frame both paths add with their own
/// recipe name and line. What must agree is the diagnostic itself.
fn body(diagnostic: &str) -> &str {
    // Anchor on the placeholder itself: the test path's frame is
    // `test placeholder error at line N:`, which a looser `"placeholder "`
    // search would match instead of the diagnostic.
    match diagnostic.find("placeholder $<") {
        Some(i) => &diagnostic[i..],
        None => diagnostic,
    }
}

#[test]
fn retired_env_prefix_answers_the_same_in_both_bodies() {
    // Before: the cook body gave the CS-0172 migration message while the test
    // body advised declaring a variable literally named `env.HOME`.
    let (cook, test) = diagnostics_for("env.HOME");
    assert!(
        !cook.is_empty() && !test.is_empty(),
        "both bodies must reject the retired prefix; cook={cook:?} test={test:?}"
    );
    assert_eq!(body(&cook), body(&test), "cook={cook:?}\ntest={test:?}");
    assert!(
        cook.contains("the `env.` prefix is retired"),
        "expected the CS-0172 migration message, got: {cook}"
    );
}

#[test]
fn recipe_member_ref_answers_the_same_in_both_bodies() {
    // Before: the cook body gave the typed RecipeMemberOutsideFanout error
    // while the test body advised declaring a variable named `c[in]`.
    let cook_src = "recipe dep\n    ingredients \"*.txt\"\n    cook \"out/$<in.stem>.o\" { cp $<in> $<out> }\n\nrecipe c\n    ingredients \"*.txt\"\n    cook \"out/x.o\" { echo \"$<dep[in]>\" > $<out> }\n";
    let test_src = "recipe dep\n    ingredients \"*.txt\"\n    cook \"out/$<in.stem>.o\" { cp $<in> $<out> }\n\nrecipe t\n    ingredients \"*.txt\"\n    test { echo \"$<dep[in]>\" }\n";
    let cook = codegen(cook_src).expect_err("cook body must reject");
    let test = codegen(test_src).expect_err("test body must reject");
    assert_eq!(body(&cook), body(&test), "cook={cook:?}\ntest={test:?}");
    assert!(
        cook.contains("only valid inside an `ingredients <probe>` fan-out body"),
        "expected the fan-out diagnostic, got: {cook}"
    );
}

#[test]
fn unknown_in_accessor_lowers_the_same_in_both_bodies() {
    // `in.bogus` is not a path accessor. The cook body has always lowered it
    // to a variable read, which reports the ident at register time. The test
    // body instead lowered it blindly to `path.bogus(_test_in)` and died with
    // a raw Lua "attempt to call a nil value (field 'bogus')".
    //
    // Asserted on the lowering rather than a diagnostic because neither body
    // rejects at codegen: both defer to the register-phase variable check, and
    // agreeing to defer is the point.
    let cook = codegen(
        "recipe c\n    ingredients \"*.txt\"\n    cook \"out/$<in.stem>.o\" { echo \"$<in.bogus>\" > $<out> }\n",
    )
    .expect("cook body lowers");
    let test = codegen(
        "recipe t\n    ingredients \"*.txt\"\n    test { echo \"$<in.bogus>\" }\n",
    )
    .expect("test body lowers");

    for (label, lua) in [("cook", &cook), ("test", &test)] {
        assert!(
            lua.contains(r#"cook.require_var("in.bogus")"#),
            "{label} body must defer the unknown ident to the variable check:\n{lua}"
        );
        assert!(
            !lua.contains("path.bogus"),
            "{label} body must not emit a call to a nonexistent accessor:\n{lua}"
        );
    }
}

#[test]
fn probe_ref_is_refused_identically_by_both_test_paths() {
    // COOK-357 deliberately does NOT make this work; COOK-360 does, by giving
    // test units the execute-time substitution cook units already have. What
    // this slice guarantees is that the plain and fan-out test paths refuse it
    // with the same diagnostic, instead of one erroring and the other claiming
    // no config block declares the probe key.
    let plain = codegen(
        "probe sys:os\n    { uname -s }\n\nrecipe t\n    test { echo \"$<sys:os>\" }\n",
    )
    .expect_err("plain test body must refuse");
    let fan_out = codegen(
        "probe sys:os\n    { uname -s }\n\nprobe list:items\n    >{ return {\"a\"} }\n\nrecipe f\n    ingredients list:items\n    test { echo \"$<sys:os>\" }\n",
    )
    .expect_err("fan-out test body must refuse");

    let strip_line = |d: &str| {
        let (head, tail) = d.split_once("at line ").expect("diagnostic names a line");
        let rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
        format!("{head}at line N{rest}")
    };
    assert_eq!(
        strip_line(&plain),
        strip_line(&fan_out),
        "plain={plain:?}\nfan_out={fan_out:?}"
    );
    assert!(
        plain.contains("no execute-time probe substitution"),
        "the diagnostic must name the real reason, got: {plain}"
    );
    assert!(
        !plain.contains("config block"),
        "a probe key must never be reported as an undeclared variable: {plain}"
    );
}

#[test]
fn a_variable_substitutes_in_an_output_pattern_as_it_does_in_a_body() {
    // COOK-357: `OutputPatternKind::Literal` used to bypass expansion, so the
    // pattern reached /bin/sh with the sigil intact and wrote a file called
    // `out/all-$`. Both surfaces now lower the same reference the same way.
    let lua = codegen(
        "config\n    var.suffix = \"dev\"\n\nrecipe r\n    ingredients \"*.c\"\n    cook \"out/all-$<suffix>.o\" { cat $<in> > $<out> }\n",
    )
    .expect("must lower");
    assert!(
        !lua.contains("out/all-$<suffix>.o"),
        "the sigil must not survive into the emitted output path:\n{lua}"
    );
    assert_eq!(
        lua.matches(r#"cook.require_var("suffix")"#).count(),
        1,
        "the output pattern must read the variable exactly as a body would:\n{lua}"
    );
}

#[test]
fn a_bare_recipe_name_is_rejected_in_any_output_pattern() {
    // COOK-357 unified the recipe set the two output-pattern callers passed:
    // the dep-driven one passed the real set, the ordinary one an EMPTY one,
    // so a bare `$<dep>` would have lowered to `cook.dep_output` in one
    // pattern and `cook.require_var` in another purely because the first
    // pattern happened to carry a dep accessor elsewhere.
    //
    // No semantics moved, because the checked path rejects the form outright
    // in both shapes. This pins that: the divergence was latent, and it stays
    // unreachable.
    let dep = "recipe dep\n    ingredients \"*.txt\"\n    cook \"out/$<in.stem>.o\" { cp $<in> $<out> }\n\n";
    let with_accessor = codegen(&format!(
        "{dep}recipe r\n    ingredients \"*.c\"\n    cook \"out/$<dep.stem>-$<dep>.o\" {{ cat $<in> > $<out> }}\n"
    ))
    .expect_err("bare recipe ref in an output pattern is rejected");
    let without_accessor = codegen(&format!(
        "{dep}recipe r\n    ingredients \"*.c\"\n    cook \"out/$<dep>.o\" {{ cat $<in> > $<out> }}\n"
    ))
    .expect_err("bare recipe ref in an output pattern is rejected");

    for (label, e) in [("with accessor", &with_accessor), ("without", &without_accessor)] {
        assert!(
            e.contains("bare recipe reference) is not allowed in an output pattern"),
            "{label}: expected the uniform rejection, got: {e}"
        );
    }
}

#[test]
fn no_generated_lua_carries_a_sigil_error_marker() {
    // The deletion check for the retired sentinel channel: an unresolvable
    // placeholder is a typed error, never a string literal smuggled through
    // the emitted Lua for a later grep to find.
    let sources = [
        "recipe r\n    ingredients \"*.c\"\n    cook \"out/a.o\" { echo $<out_0> > $<out> }\n",
        "recipe r\n    ingredients \"*.c\"\n    cook \"out/$<render[]>.o\" { cat $<in> > $<out> }\n",
        "recipe r\n    ingredients \"*.c\"\n    cook \"out/a.o\" \"out/b.o\" { gen $<out> }\n",
    ];
    for src in sources {
        match codegen(src) {
            Ok(lua) => assert!(
                !lua.contains("SIGIL_ERROR"),
                "emitted Lua must never carry the marker:\n{lua}"
            ),
            Err(e) => assert!(
                !e.contains("SIGIL_ERROR"),
                "diagnostics must be typed, not marker text: {e}"
            ),
        }
    }
}

#[test]
fn codegen_is_the_only_public_lowering_entry_point() {
    // COOK-357 collapsed four entry points into one. `generate_checked`
    // returns the warnings alongside the Lua so callers stop lowering the
    // whole Cookfile twice to collect them.
    let cookfile = cook_lang::parse(
        "recipe empty\n\nrecipe r\n    ingredients \"*.c\"\n    cook \"out/a.o\" { echo $<empty> > $<out> }\n",
    )
    .expect("parses");
    let names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
    let (lua, warnings) = cook_luagen::generate_checked(&cookfile, &names).expect("must lower");
    assert!(!lua.is_empty(), "lowering must produce Lua");
    assert!(
        warnings.iter().any(|w| w.contains("empty")),
        "the § 5.5 warning must ride along with the lowering, got: {warnings:?}"
    );
}

#[test]
fn the_resolver_is_the_only_answer_to_what_an_ident_is() {
    // A guard on the invariant itself rather than on any one symptom: no
    // lowering or validation path may re-derive an ident's shape by hand.
    // These spellings are how the plate/test chain and the ad-hoc classifiers
    // drifted from `resolve` in the first place.
    let template = code_only(include_str!("../src/template.rs"));
    for spelling in [
        r#"ident == "in" || ident.starts_with("in.")"#,
        r#"ident.strip_prefix("in.")"#,
        r#"ident == "out""#,
        r#"ident.starts_with("out.")"#,
    ] {
        assert!(
            !template.contains(spelling),
            "template.rs re-derives an ident shape by hand: {spelling}"
        );
    }
    assert!(
        !template.contains("SIGIL_ERROR"),
        "the sentinel error channel must stay deleted"
    );

    // And the probe-ref walker exists once, in the sigil grammar.
    let resolver = code_only(include_str!("../src/resolver.rs"));
    assert!(
        !resolver.contains("cook.probes.get(\\\""),
        "resolver.rs must not build the probe access expression itself"
    );
    let _ = BTreeSet::<String>::new();
}

/// Drop comment lines so the guard above cannot be satisfied — or tripped — by
/// prose. The comments in these files quote the retired spellings on purpose,
/// to record what drifted and why.
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}
