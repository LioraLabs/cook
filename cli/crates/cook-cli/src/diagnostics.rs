//! Presentation-layer cleanup for user-facing diagnostics.

/// Whether `-v/--verbose` (or `COOK_BACKTRACE=1` set directly) requests that
/// raw Lua stack tracebacks be preserved in error output.
pub fn backtrace_enabled() -> bool {
    std::env::var("COOK_BACKTRACE").map(|v| v == "1").unwrap_or(false)
}

/// Cleans a Lua-originated error string for display: cuts the mlua
/// traceback (unless `keep_traceback`) and drops the "lua error: " /
/// "runtime error: " wrapper prefixes, preserving a leading "[recipe] " tag.
pub fn sanitize_error(msg: &str, keep_traceback: bool) -> String {
    cook_contracts::lua_error::sanitize(msg, keep_traceback)
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
#[path = "tests/diagnostics_tests.rs"]
mod tests;
