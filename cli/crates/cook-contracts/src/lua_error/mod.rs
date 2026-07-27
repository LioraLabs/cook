/// Remove Lua runtime wrappers and optionally its traceback.
pub fn sanitize(message: &str, keep_traceback: bool) -> String {
    let message = if keep_traceback {
        message
    } else {
        message
            .split_once("\nstack traceback:")
            .map_or(message, |(before, _)| before)
    };
    let (tag, rest) = if message.starts_with('[') {
        message
            .find("] ")
            .map_or(("", message), |end| message.split_at(end + 2))
    } else {
        ("", message)
    };
    let rest = rest.strip_prefix("lua error: ").unwrap_or(rest);
    let rest = rest.strip_prefix("runtime error: ").unwrap_or(rest);
    format!("{tag}{rest}")
}

#[cfg(test)]
#[path = "tests/lua_error_tests.rs"]
mod tests;
