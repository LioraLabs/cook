//! COOK-490 / CS-0239 — `gather $<recipe>` lowers to a member fan-out over
//! the named recipe's declared output paths.

use cook_luagen::{dep_ref::extract_recipe_names, generate_checked};

const PRODUCER: &str = "recipe gen\n    gather \"src/*.conf.in\"\n    cook \"build/$<in.stem>.conf\" { render $<in> > $<out> }\n\n";

fn lower(source: &str) -> String {
    let cookfile = cook_lang::parse(source).unwrap();
    let names = extract_recipe_names(&cookfile);
    generate_checked(&cookfile, &names).unwrap().0
}

fn reject(source: &str) -> String {
    let cookfile = cook_lang::parse(source).unwrap();
    let names = extract_recipe_names(&cookfile);
    generate_checked(&cookfile, &names)
        .expect_err("expected a codegen rejection")
        .to_string()
}

/// The motivating gap: a per-upstream-output `test` step, which has no output
/// pattern and therefore could never declare a dep-driven driver.
#[test]
fn cs0239_members_come_from_the_named_recipes_output_list() {
    let lua = lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    test {{ validate $<in> }}\n"
    ));
    assert!(
        lua.contains("local _items = cook.dep_output_list(\"gen\")"),
        "member set must come from the referent's declared outputs:\n{lua}"
    );
}

/// §10.6: the name reference IS the edge, and it is a whole-recipe edge, not
/// a per-step-group one. It also fixes registration order — the register pass
/// runs bodies in `requires` topological order, and `cook.dep_output_list`
/// answers only for a recipe already registered.
#[test]
fn cs0239_the_source_name_establishes_the_cross_recipe_edge() {
    let lua = lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    test {{ validate $<in> }}\n"
    ));
    let meta = lua
        .lines()
        .find(|l| l.contains("\"check\""))
        .expect("check's registration line");
    assert!(
        meta.contains("requires = {\"gen\"}"),
        "the gather source must pull its referent into the closure:\n{meta}"
    );
}

/// COOK-355's stale-artifact finding: a member unit keyed on a path STRING
/// and not on the file's content replays a pass over an artifact that has
/// since gone bad. The member path must be a declared file input.
#[test]
fn cs0239_each_member_path_is_a_declared_input_of_its_unit() {
    let cook = lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    cook \"out/$<in.stem>.ok\" {{ validate $<in> > $<out> }}\n"
    ));
    assert!(
        cook.contains("inputs = {cook.member_to_string(item)}"),
        "fan-out cook unit must declare its member path as an input:\n{cook}"
    );

    let test = lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    test {{ validate $<in> }}\n"
    ));
    assert!(
        test.contains("_test_src = {cook.member_to_string(item)}"),
        "a fan-out test with no preceding cook step must key on the member path:\n{test}"
    );
}

/// A recipe source is not a probe. Emitting a `__member_source` descriptor
/// would send the register pre-pass looking for a probe named `gen`.
#[test]
fn cs0239_no_probe_pre_pass_descriptor_is_emitted() {
    let lua = lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    test {{ validate $<in> }}\n"
    ));
    assert!(
        !lua.contains("__member_source"),
        "a recipe source has no probe for the pre-pass to resolve:\n{lua}"
    );
}

/// CS-0234 scoped the single-quote law to DATA members. A recipe's outputs
/// are path members — a string, never composite — so the law does not reach
/// them, exactly as it does not reach a named-`files` gather.
#[test]
fn cs0239_path_members_are_outside_the_data_member_quoting_law() {
    lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    test {{ node --check $<in> }}\n"
    ));
    lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    cook \"out/$<in.stem>.ok\" {{ validate $<in> > $<out> }}\n"
    ));
}

#[test]
fn cs0239_a_source_naming_no_recipe_is_rejected() {
    let error = reject(&format!(
        "{PRODUCER}recipe check\n    gather $<nope>\n    test {{ validate $<in> }}\n"
    ));
    assert!(
        error.contains("nope") && error.contains("no such recipe is declared in this Cookfile"),
        "unexpected diagnostic: {error}"
    );
}

/// §8.2 Form 3 constraint 2 scopes the source to the current Cookfile. A
/// qualified reference resolves fine everywhere else, so §10.2.4 requires the
/// refusal to name the POSITION rather than report an undeclared recipe —
/// otherwise the author hunts a typo that is not there. COOK-512 tracks
/// admitting it.
#[test]
fn cs0239_a_qualified_reference_is_refused_by_position_not_as_a_typo() {
    let error = reject(&format!(
        "{PRODUCER}recipe check\n    gather $<lib.gen>\n    test {{ validate $<in> }}\n"
    ));
    assert!(
        error.contains("lib.gen") && error.contains("qualified") && error.contains("this position"),
        "the diagnostic must name the position, not call the recipe undeclared: {error}"
    );
    assert!(
        !error.contains("no such recipe"),
        "a qualified reference is declared elsewhere; do not report it missing: {error}"
    );
}

#[test]
fn cs0239_a_recipe_cannot_gather_its_own_outputs() {
    let error = reject("recipe loop\n    gather $<loop>\n    test { validate $<in> }\n");
    assert!(
        error.contains("loop") && error.contains("its own"),
        "unexpected diagnostic: {error}"
    );
}

/// CS-0155's rejection reaches this form too — an aggregate over `gen`'s
/// outputs is already spelled `$<gen>` in a body, with no `gather` at all —
/// but its diagnostic must stop telling a recipe-source author about data
/// members.
#[test]
fn cs0239_an_all_literal_first_cook_step_is_rejected_in_its_own_terms() {
    let lua = lower(&format!(
        "{PRODUCER}recipe check\n    gather $<gen>\n    cook \"out/all.ok\" {{ validate $<in> > $<out> }}\n"
    ));
    assert!(
        lua.contains("fan out first") && lua.contains("$<gen>"),
        "the rejection must name the aggregate spelling it redirects to:\n{lua}"
    );
    assert!(
        !lua.contains("data members are not a collected file set"),
        "a recipe source binds path members, not data members:\n{lua}"
    );
}
