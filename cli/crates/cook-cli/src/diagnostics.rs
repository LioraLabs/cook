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

/// Best-effort source location extraction from a rendered diagnostic.
pub fn extract_location(msg: &str) -> (Option<String>, Option<usize>) {
    if let Some((head, _)) = msg.split_once(": ") {
        let mut parts = head.rsplitn(2, ':');
        if let (Some(line), Some(path)) = (parts.next(), parts.next()) {
            if path.ends_with("Cookfile") {
                if let Ok(n) = line.parse::<usize>() {
                    return (Some(path.to_string()), Some(n));
                }
            }
        }
    }

    if let Some(i) = msg.find("line ") {
        let digits: String = msg[i + 5..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse::<usize>() {
            return (Some("Cookfile".to_string()), Some(n));
        }
    }

    (None, None)
}

pub fn json_diagnostic(code: &str, msg: &str) -> String {
    let (file, line) = extract_location(msg);
    serde_json::json!({
        "type": "diagnostic",
        "code": code,
        "file": file,
        "line": line,
        "message": msg,
    })
    .to_string()
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
}
