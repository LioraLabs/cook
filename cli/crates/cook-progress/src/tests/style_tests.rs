use super::*;

#[test]
fn compile_kind_maps_to_compiled_verb() {
    let v = verb_for(LineKind::NodeCompleted, NodeKind::Compile);
    assert_eq!(v.text, "Compiled");
    assert!(v.bold);
    assert_eq!(v.color, VerbColor::Default);
}

#[test]
fn cooked_is_default_for_completed_node_with_no_kind_info() {
    let v = verb_for(LineKind::NodeCompleted, NodeKind::Cooked);
    assert_eq!(v.text, "Cooked");
}

#[test]
fn cached_uses_dim_color() {
    let v = verb_for(LineKind::NodeCached, NodeKind::Cooked);
    assert_eq!(v.text, "Cached");
    assert_eq!(v.color, VerbColor::Dim);
}

#[test]
fn failed_is_bold_red() {
    let v = verb_for(LineKind::NodeFailed, NodeKind::Cooked);
    assert!(v.bold);
    assert_eq!(v.color, VerbColor::Red);
}

#[test]
fn finished_is_bold_green() {
    let v = verb_for(LineKind::RecipeFinished, NodeKind::Cooked);
    assert!(v.bold);
    assert_eq!(v.color, VerbColor::Green);
}

#[test]
fn status_bar_verb_is_cooking() {
    let v = verb_for(LineKind::StatusBar, NodeKind::Cooked);
    assert_eq!(v.text, "Cooking");
    assert!(v.bold);
}

#[test]
fn format_verb_pads_to_12_cols() {
    let v = verb_for(LineKind::NodeCompleted, NodeKind::Cooked);
    let s = format_verb(v, false);
    assert_eq!(s.chars().count(), 12);
    assert!(s.ends_with("Cooked"));
}

#[test]
fn format_verb_with_color_wraps_in_ansi() {
    let v = verb_for(LineKind::NodeFailed, NodeKind::Cooked);
    let s = format_verb(v, true);
    assert!(s.starts_with("\x1b[1m\x1b[31m"));
    assert!(s.ends_with("\x1b[0m"));
}
