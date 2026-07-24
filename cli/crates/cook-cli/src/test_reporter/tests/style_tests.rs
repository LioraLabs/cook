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
