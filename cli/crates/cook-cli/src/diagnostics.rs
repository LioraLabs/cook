//! Presentation-layer cleanup for user-facing diagnostics.

/// Whether `-v/--verbose` (or `COOK_BACKTRACE=1` set directly) requests that
/// raw Lua stack tracebacks be preserved in error output.
pub fn backtrace_enabled() -> bool {
    std::env::var(cook_contracts::lua_error::BACKTRACE_ENV).map(|v| v == "1").unwrap_or(false)
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

/// The one stderr subscriber: cook's own warnings and errors, in the same
/// `cook: warning: ...` form as the CLI's direct `eprintln!`s. Third-party
/// crates (h2, ...) also use `tracing`; their events are not cook's to print.
/// The sink is stderr in the process and a buffer under test.
struct StderrWarnings<W>(std::sync::Mutex<W>);

/// Collects an event's `message` and every other field as `name=value`.
#[derive(Default)]
struct Fields {
    message: String,
    rest: Vec<String>,
}

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if f.name() == "message" {
            let _ = write!(self.message, "{v:?}");
        } else {
            self.rest.push(format!("{}={v:?}", f.name()));
        }
    }
}

/// One event as the line the user sees, or `None` when it is not cook's to show.
pub fn render_event(event: &tracing::Event<'_>) -> Option<String> {
    let meta = event.metadata();
    if !is_cook_diagnostic(meta) {
        return None;
    }
    let mut fields = Fields::default();
    event.record(&mut fields);
    let label = if *meta.level() == tracing::Level::ERROR { "error" } else { "warning" };
    let text: Vec<_> = Some(fields.message).filter(|m| !m.is_empty()).into_iter().chain(fields.rest).collect();
    Some(format!("cook: {label}: {}\n", text.join(" ")))
}

/// `cook`, `cook_*` crates and `cook::` modules; not `cookie` and the like.
fn is_cook_target(target: &str) -> bool {
    target == "cook" || target.starts_with("cook_") || target.starts_with("cook::")
}

fn is_cook_diagnostic(meta: &tracing::Metadata<'_>) -> bool {
    *meta.level() <= tracing::Level::WARN && is_cook_target(meta.target())
}

impl<W: std::io::Write + 'static> tracing::Subscriber for StderrWarnings<W> {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        is_cook_diagnostic(meta)
    }
    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::WARN)
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if let Some(line) = render_event(event) {
            // One write under the lock: concurrent units never interleave a line.
            let _ = self.0.lock().unwrap().write_all(line.as_bytes());
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Installs the stderr subscriber for the process; a second call is a no-op.
pub fn install_stderr_subscriber() {
    let _ = tracing::subscriber::set_global_default(StderrWarnings(std::sync::Mutex::new(std::io::stderr())));
}

#[cfg(test)]
#[path = "tests/diagnostics_tests.rs"]
mod tests;
