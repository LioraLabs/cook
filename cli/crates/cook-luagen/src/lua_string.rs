pub(crate) fn escape_lua_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub(crate) fn wrap_lua_string(s: &str) -> String {
    if s.contains("]]") {
        format!("[=[{}]=]", s)
    } else {
        format!("[[{}]]", s)
    }
}
