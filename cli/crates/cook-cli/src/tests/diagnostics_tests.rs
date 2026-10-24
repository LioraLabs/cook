use super::*;

#[test]
fn strips_traceback_and_wrappers() {
    let raw = "lua error: runtime error: Cookfile:3: attempt to call a nil value (global 'OUTDIR')\nstack traceback:\n\t[C]: in global 'OUTDIR'\n\tCookfile:3: in function '__cook_run_config_blocks'";
    assert_eq!(
        sanitize_error(raw, false),
        "Cookfile:3: attempt to call a nil value (global 'OUTDIR')"
    );
}

#[test]
fn preserves_recipe_tag() {
    let raw = "[boom] lua error: runtime error: Cookfile:2: kaboom\nstack traceback:\n\t[C]: in ?";
    assert_eq!(sanitize_error(raw, false), "[boom] Cookfile:2: kaboom");
    assert_eq!(
        sanitize_error(raw, false),
        cook_contracts::lua_error::sanitize(raw, false)
    );
}

#[test]
fn backtrace_optin_keeps_traceback() {
    let raw = "lua error: runtime error: Cookfile:3: boom\nstack traceback:\n\tx";
    let s = sanitize_error(raw, true);
    assert!(s.contains("stack traceback:"));
    assert!(s.starts_with("Cookfile:3: boom"));
}

#[test]
fn plain_messages_pass_through() {
    assert_eq!(sanitize_error("recipe not found: zzz", false), "recipe not found: zzz");
}

#[test]
fn extracts_leading_cookfile_location() {
    assert_eq!(
        extract_location("Cookfile:3: attempt to call a nil value"),
        (Some("Cookfile".to_string()), Some(3))
    );
    assert_eq!(
        extract_location("sub/Cookfile:7: kaboom"),
        (Some("sub/Cookfile".to_string()), Some(7))
    );
}

#[test]
fn extracts_parse_error_line_location() {
    assert_eq!(
        extract_location("parse error: line 2: config values are Lua assignments"),
        (Some("Cookfile".to_string()), Some(2))
    );
}

#[test]
fn locationless_messages_return_none() {
    assert_eq!(extract_location("recipe not found: zzz"), (None, None));
}

/// Runs `emit` under the real subscriber writing to a buffer; returns what it wrote.
fn rendered(emit: impl FnOnce()) -> String {
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let out = Shared(Arc::new(Mutex::new(vec![])));
    tracing::subscriber::with_default(StderrWarnings(Mutex::new(out.clone())), emit);
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    text
}

#[test]
fn warn_and_error_render_as_cook_lines_with_extra_fields_kept() {
    let lines = rendered(|| {
        tracing::warn!("store down: {}", 7);
        tracing::error!(unit = "app", "boom");
        tracing::info!("not shown");
        tracing::warn!(target: "h2::codec", "third party");
        tracing::warn!(target: "cookie::jar", "lookalike");
    });
    assert_eq!(lines, "cook: warning: store down: 7\ncook: error: boom unit=\"app\"\n");
}

#[test]
fn an_event_with_fields_and_no_message_has_no_double_space() {
    assert_eq!(rendered(|| tracing::warn!(unit = "app", n = 2)), "cook: warning: unit=\"app\" n=2\n");
}
