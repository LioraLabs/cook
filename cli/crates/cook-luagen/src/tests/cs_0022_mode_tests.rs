use super::*;
use cook_lang::ast::{CookStep, OutputPattern, Body};

fn step(outputs: &[&str], body: Option<Body>) -> CookStep {
    CookStep {
        outputs: outputs
            .iter()
            .map(|s| OutputPattern::Quoted((*s).to_string()))
            .collect(),
        body,
        disposition: Default::default(),
    }
}

fn empty_recipes() -> BTreeSet<String> {
    BTreeSet::new()
}

#[test]
fn literal_output_is_many_to_one_regardless_of_body() {
    // A literal output pattern → ManyToOne, even if the body contains $<in>.
    let s = step(
        &["build/app"],
        Some(Body::ShellBlock(vec!["gcc $<in>".into()])),
    );
    assert!(matches!(
        cook_step_mode_with_names(&s, &empty_recipes()),
        CookMode::ManyToOne
    ));
}

#[test]
fn in_accessor_output_is_one_to_one() {
    let s = step(
        &["build/$<in.stem>.o"],
        Some(Body::ShellBlock(vec!["gcc $<in> -o $<out>".into()])),
    );
    assert!(matches!(
        cook_step_mode_with_names(&s, &empty_recipes()),
        CookMode::OneToOne
    ));
}

#[test]
fn lib_accessor_output_is_one_to_one_dep_driven() {
    // With recipe-name context, `$<libmath.stem>` is recognised as a
    // dep-driven pattern → OneToOne. Without names it is Literal →
    // ManyToOne. Both outcomes are acceptable here; the exhaustive
    // check is in resolves_recipe_accessor / sigil tests.
    let s = step(
        &["build/$<libmath.stem>.x"],
        Some(Body::ShellBlock(vec!["echo $<in>".into()])),
    );
    let mut names = BTreeSet::new();
    names.insert("libmath".to_string());
    assert!(matches!(
        cook_step_mode_with_names(&s, &names),
        CookMode::OneToOne
    ));
    assert!(matches!(
        cook_step_mode_with_names(&s, &empty_recipes()),
        CookMode::ManyToOne
    ));
}

#[test]
fn multi_output_literal_is_block_step() {
    let s = step(
        &["a.js", "a.wasm"],
        Some(Body::ShellBlock(vec!["gen".into()])),
    );
    assert!(matches!(
        cook_step_mode_with_names(&s, &empty_recipes()),
        CookMode::BlockStep
    ));
}

#[test]
fn declaration_only_no_body() {
    let s = step(&["x"], None);
    assert!(matches!(
        cook_step_mode_with_names(&s, &empty_recipes()),
        CookMode::DeclarationOnly
    ));
}
