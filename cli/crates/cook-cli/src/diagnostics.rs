//! Presentation-layer cleanup for user-facing diagnostics.
//! A twin of the wrapper-stripping/traceback truncation lives in
//! cook-luaotp/src/pool.rs for execute-phase WorkResult errors — keep in sync.

/// Whether `-v/--verbose` (or `COOK_BACKTRACE=1` set directly) requests that
/// raw Lua stack tracebacks be preserved in error output.
pub fn backtrace_enabled() -> bool {
    std::env::var("COOK_BACKTRACE").map(|v| v == "1").unwrap_or(false)
}

/// Cleans a Lua-originated error string for display: cuts the mlua
/// traceback (unless `keep_traceback`) and drops the "lua error: " /
/// "runtime error: " wrapper prefixes, preserving a leading "[recipe] " tag.
pub fn sanitize_error(msg: &str, keep_traceback: bool) -> String {
    let mut m = msg.to_string();
    if !keep_traceback {
        if let Some(pos) = m.find("\nstack traceback:") {
            m.truncate(pos);
        }
    }
    let (tag, rest): (&str, &str) = if m.starts_with('[') {
        match m.find("] ") {
            Some(i) => (&m[..i + 2], &m[i + 2..]),
            None => ("", m.as_str()),
        }
    } else {
        ("", m.as_str())
    };
    let rest = rest.strip_prefix("lua error: ").unwrap_or(rest);
    let rest = rest.strip_prefix("runtime error: ").unwrap_or(rest);
    format!("{tag}{rest}")
}

#[cfg(test)]
mod tests {
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
}
