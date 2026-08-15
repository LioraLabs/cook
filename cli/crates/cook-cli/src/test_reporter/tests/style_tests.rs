use super::*;

#[test]
fn resolve_always_forces_color() {
    assert!(resolve_color_choice("always", None, false));
    assert!(resolve_color_choice("always", Some("1"), false));
}

#[test]
fn resolve_never_forces_no_color() {
    assert!(!resolve_color_choice("never", None, true));
    }

    #[test]
    fn resolve_auto_respects_no_color_env() {
        assert!(!resolve_color_choice("auto", Some("1"), true));
}

#[test]
fn resolve_auto_falls_back_to_tty() {
    assert!(resolve_color_choice("auto", None, true));
    assert!(!resolve_color_choice("auto", None, false));
}

#[test]
fn resolve_auto_treats_empty_no_color_as_unset() {
    // Per no-color.org: a NO_COLOR with empty value is unset.
    assert!(resolve_color_choice("auto", Some(""), true));
}

#[test]
fn style_wraps_when_colored() {
    let s = Style::new(true);
    assert_eq!(s.green("ok"), "\x1b[32mok\x1b[0m");
    assert_eq!(s.bold_red("FAILED"), "\x1b[1;31mFAILED\x1b[0m");
}

#[test]
fn style_passes_through_when_uncolored() {
    let s = Style::new(false);
    assert_eq!(s.green("ok"), "ok");
    assert_eq!(s.bold_red("FAILED"), "FAILED");
}

/// COOK-411: `progress.rs` resolved colour itself and disagreed with this
/// function on exactly two inputs, so a single `cook test` run answered
/// differently for its progress output and its test report. Both callers share
/// this function now; these are the two cases that differed.
#[test]
fn the_two_inputs_the_progress_renderer_used_to_answer_differently() {
    // `--color=always` beats NO_COLOR: an explicit flag wins over the
    // environment. progress.rs returned false here.
    assert!(
        resolve_color_choice("always", Some("1"), true),
        "an explicit --color=always must win over NO_COLOR"
    );

    // NO_COLOR="" is NOT set, per no-color.org ("present and not an empty
    // string"). progress.rs used `is_ok()` and treated it as set.
    assert!(
        resolve_color_choice("auto", Some(""), true),
        "an empty NO_COLOR must not disable colour"
    );
}
